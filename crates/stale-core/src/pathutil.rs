use std::path::{Path, PathBuf};

/// Expands `%VAR%` references; unknown variables are left as-is.
pub fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match std::env::var(&after[..end]) {
                Ok(val) => {
                    out.push_str(&val);
                    rest = &after[end + 1..];
                }
                Err(_) => {
                    // Unknown: keep the opening '%' literally and rescan from the next char.
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `%ProgramFiles%` -> `%ProgramFiles(x86)%` and `%CommonProgramFiles%` ->
/// `%CommonProgramFiles(x86)%` (case-insensitive): what a 32-bit program's value means.
pub fn wow64_vars(s: &str) -> String {
    const MAP: [(&str, &str); 2] =
        [("%PROGRAMFILES%", "%ProgramFiles(x86)%"), ("%COMMONPROGRAMFILES%", "%CommonProgramFiles(x86)%")];
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'scan: while i < s.len() {
        for (from, to) in MAP {
            if s.get(i..i + from.len()).is_some_and(|w| w.eq_ignore_ascii_case(from)) {
                out.push_str(to);
                i += from.len();
                continue 'scan;
            }
        }
        let c = s[i..].chars().next().unwrap_or_default();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// [`expand_env`] for a value read in the 32-bit registry view (under `WOW6432Node`):
/// `%ProgramFiles%` and `%CommonProgramFiles%` name the x86 folders ([`wow64_vars`]).
pub fn expand_env_wow64(s: &str) -> String {
    expand_env(&wow64_vars(s))
}

/// Extracts the executable path from a command line (quotes stripped, env expanded).
pub fn first_path_in_command(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    let raw = if let Some(rest) = cmd.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else {
        let token = cmd.split_whitespace().next().unwrap_or("");
        let lower = cmd.to_ascii_lowercase();
        match lower.find(".exe") {
            Some(i) if !token.to_ascii_lowercase().ends_with(".exe") => &cmd[..i + 4],
            _ => token,
        }
    };
    let raw = raw.trim();
    if raw.is_empty() {
        None
    } else {
        Some(PathBuf::from(expand_env(raw)))
    }
}

/// The executable of a command line by the CreateProcess rule, for paths that may hold unquoted
/// spaces (`\??\C:\ProgramData\Vendor\Some Dir\x.sys`): a quoted path is taken as is; otherwise
/// the longest space-separated prefix that `is_file` (an existing file, never a folder), else the
/// whole string when it ends in a file extension, else [`first_path_in_command`]. Environment
/// variables are expanded.
pub fn resolve_unquoted_path(cmd: &str, is_file: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let cmd = expand_env(cmd.trim());
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return None;
    }
    if cmd.starts_with('"') {
        return first_path_in_command(cmd);
    }
    let ends = cmd.match_indices(' ').map(|(i, _)| i).chain(std::iter::once(cmd.len())).collect::<Vec<_>>();
    for &end in ends.iter().rev() {
        let candidate = cmd[..end].trim_end();
        if !candidate.is_empty() && is_file(Path::new(candidate)) {
            return Some(PathBuf::from(candidate));
        }
    }
    if has_file_extension(Path::new(cmd)) {
        return Some(PathBuf::from(cmd));
    }
    first_path_in_command(cmd)
}

/// A 1-4 character alphanumeric extension with a letter (`.exe`, `.sys`); `exe -k run` is none.
fn has_file_extension(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        (1..=4).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric()) && e.chars().any(|c| c.is_ascii_alphabetic())
    })
}

/// `s` as UTF-16 with a terminating NUL, for Win32 string arguments (`PCWSTR`).
pub fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Uppercase drive letter of a path; `None` for UNC or relative paths.
pub fn drive_letter(p: &Path) -> Option<char> {
    let s = p.to_str()?;
    let mut chars = s.chars();
    let letter = chars.next()?;
    (letter.is_ascii_alphabetic() && chars.next() == Some(':')).then(|| letter.to_ascii_uppercase())
}

/// `p` in its long form (`C:\PROGRA~1\X` -> `C:\Program Files\X`) from `GetLongPathNameW`;
/// `p` itself when the call fails (a missing path, an unreadable folder).
pub fn long_path_name(p: &Path) -> PathBuf {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetLongPathNameW;
    let input = HSTRING::from(p.as_os_str());
    let mut buf = vec![0u16; 512];
    for _ in 0..2 {
        // SAFETY: `input` is NUL-terminated and `buf` is a valid, writable buffer.
        let len = unsafe { GetLongPathNameW(&input, Some(&mut buf)) } as usize;
        if len == 0 {
            break;
        }
        if len < buf.len() {
            return PathBuf::from(String::from_utf16_lossy(&buf[..len]));
        }
        buf = vec![0u16; len];
    }
    p.to_path_buf()
}

/// Uppercase, backslash-separated, no trailing separator (`C:\` -> `C:`).
pub fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', r"\").to_uppercase();
    s.trim_end_matches('\\').to_string()
}

/// Folders (normalized) that many programs share. Windows and ProgramData come from the system
/// ([`crate::paths`]), never from `%SystemRoot%`/`%ProgramData%`; the per-user and Program Files
/// folders from the environment, on each call.
fn shared_folders() -> Vec<String> {
    const VARS: [&str; 6] = [
        "%ProgramFiles%",
        "%ProgramFiles(x86)%",
        "%LOCALAPPDATA%",
        r"%LOCALAPPDATA%\Programs",
        "%APPDATA%",
        "%USERPROFILE%",
    ];
    VARS.iter()
        .map(|v| expand_env(v))
        .filter(|s| !s.contains('%'))
        .map(|s| norm(Path::new(&s)))
        .chain(system_shared().iter().cloned())
        .collect()
}

/// Windows, its System32 and SysWOW64, and ProgramData ([`norm`]ed), from the system API; read
/// once per process.
fn system_shared() -> &'static [String] {
    static SYSTEM: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    SYSTEM.get_or_init(|| {
        let windows = crate::paths::windows_dir();
        [windows.clone(), windows.join("System32"), windows.join("SysWOW64"), crate::paths::program_data_dir()]
            .iter()
            .map(|p| norm(p))
            .collect()
    })
}

/// The Users folder ([`crate::paths::user_profiles_root`]), normalized; read once per process.
fn users_root() -> Option<String> {
    static USERS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    Some(USERS.get_or_init(|| norm(&crate::paths::user_profiles_root())).clone()).filter(|n| !n.is_empty())
}

/// `\Program Files`-style folders (normalized, without drive) that are shared on any drive.
const SHARED_ON_ANY_DRIVE: [&str; 4] =
    [r"\PROGRAM FILES", r"\PROGRAM FILES (X86)", r"\PROGRAM FILES\COMMON FILES", r"\PROGRAM FILES (X86)\COMMON FILES"];

/// True for drive roots and for the shared folders that hold many programs (Program Files and
/// its Common Files on any drive, ProgramData, AppData roots, Windows and its System32/SysWOW64,
/// the Users folder and each profile root). Their subfolders are not shared roots. An empty
/// path counts as shared so it never claims anything.
pub fn is_shared_root(p: &Path) -> bool {
    let n = norm(p);
    let is_drive_root = n.len() == 2 && drive_letter(Path::new(&n)).is_some();
    if n.is_empty() || is_drive_root || shared_folders().contains(&n) {
        return true;
    }
    if drive_letter(Path::new(&n)).is_some() && n.get(2..).is_some_and(|rest| SHARED_ON_ANY_DRIVE.contains(&rest)) {
        return true;
    }
    users_root().is_some_and(|users| {
        n == users || n.strip_prefix(&users).and_then(|r| r.strip_prefix('\\')).is_some_and(|r| !r.contains('\\'))
    })
}

/// Case-insensitive "`exe` lies inside `install`"; a drive-less `exe` (volume-stripped
/// Prefetch path such as `\PROGRAM FILES\X.EXE`) is compared against `install` minus its drive.
pub fn exe_under(exe: &Path, install: &Path) -> bool {
    normed_under(&norm(exe), &norm(install))
}

/// [`exe_under`] for paths that are already [`norm`]ed.
pub(crate) fn normed_under(exe: &str, install: &str) -> bool {
    let install = match drive_letter(Path::new(install)) {
        Some(_) if exe.starts_with('\\') => &install[2..],
        _ => install,
    };
    !install.is_empty() && exe.strip_prefix(install).is_some_and(|rest| rest.len() > 1 && rest.starts_with('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_path(var: &str) -> PathBuf {
        PathBuf::from(std::env::var(var).unwrap())
    }

    #[test]
    fn norm_uppercases_and_trims_separators() {
        assert_eq!(norm(Path::new(r"c:\Program Files\Foo\")), r"C:\PROGRAM FILES\FOO");
        assert_eq!(norm(Path::new("c:/a/b")), r"C:\A\B");
        assert_eq!(norm(Path::new(r"C:\")), "C:");
    }

    #[test]
    fn shared_roots_are_detected() {
        let _env = crate::env_lock();
        assert!(is_shared_root(Path::new(r"C:\")));
        assert!(is_shared_root(Path::new("d:")));
        let vars = ["ProgramFiles", "ProgramFiles(x86)", "ProgramData", "LOCALAPPDATA", "APPDATA", "SystemRoot", "USERPROFILE"];
        for var in vars {
            let p = env_path(var);
            assert!(is_shared_root(&p), "{var}");
            let lower_slash = format!(r"{}\", p.display()).to_lowercase();
            assert!(is_shared_root(Path::new(&lower_slash)), "{var} lowercase with trailing slash");
            assert!(!is_shared_root(&p.join("Foo")), "{var} child");
        }
        let programs = env_path("LOCALAPPDATA").join("Programs");
        assert!(is_shared_root(&programs));
        assert!(!is_shared_root(&programs.join("Foo")));
        let users = PathBuf::from(format!(r"{}\Users", std::env::var("SystemDrive").unwrap()));
        assert!(is_shared_root(&users));
        assert!(is_shared_root(&users.join("SomeoneElse")));
        assert!(!is_shared_root(&users.join("SomeoneElse").join("Tools")));
        assert!(is_shared_root(Path::new("")));
    }

    #[test]
    fn program_files_on_any_drive_and_system_folders_are_shared() {
        let _env = crate::env_lock();
        for p in [
            r"D:\Program Files",
            r"e:\program files (x86)\",
            r"C:\Program Files\Common Files",
            r"D:\Program Files (x86)\Common Files",
        ] {
            assert!(is_shared_root(Path::new(p)), "{p}");
        }
        let windows = env_path("SystemRoot");
        assert!(is_shared_root(&windows.join("System32")));
        assert!(is_shared_root(&windows.join("SysWOW64")));
        assert!(!is_shared_root(Path::new(r"D:\Program Files\Foo")));
        assert!(!is_shared_root(Path::new(r"D:\Program Files\Common Files\Foo")));
        assert!(!is_shared_root(&windows.join("System32").join("Foo")));
    }

    #[test]
    fn system_shared_folders_are_read_once() {
        let (a, b) = (system_shared(), system_shared());
        assert!(std::ptr::eq(a, b), "cached");
        assert_eq!(a[0], norm(&crate::paths::windows_dir()));
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn shared_folders_ignore_env_override() {
        let _env = crate::env_lock();
        let windows = crate::paths::windows_dir();
        let program_data = crate::paths::program_data_dir();
        let users = crate::paths::user_profiles_root();
        let saved: Vec<_> = ["SystemRoot", "ProgramData", "SystemDrive"].map(|v| (v, std::env::var_os(v))).into();
        std::env::set_var("SystemRoot", r"Q:\EvilWin");
        std::env::set_var("ProgramData", r"Q:\EvilData");
        std::env::set_var("SystemDrive", "Q:");
        let got = (
            is_shared_root(&windows),
            is_shared_root(&windows.join("System32")),
            is_shared_root(&program_data),
            is_shared_root(&users.join("SomeoneElse")),
            is_shared_root(Path::new(r"Q:\EvilWin\System32")),
            is_shared_root(Path::new(r"Q:\EvilData")),
            is_shared_root(Path::new(r"Q:\Users\X")),
        );
        for (var, value) in saved {
            match value {
                Some(v) => std::env::set_var(var, v),
                None => std::env::remove_var(var),
            }
        }
        assert_eq!(got, (true, true, true, true, false, false, false));
    }

    #[test]
    fn exe_under_handles_volume_stripped_paths() {
        assert!(exe_under(Path::new(r"\PROGRAM FILES\FOO\FOO.EXE"), Path::new(r"C:\Program Files\Foo")));
        assert!(!exe_under(Path::new(r"C:\Program Files\Foobar\x.exe"), Path::new(r"C:\Program Files\Foo")));
    }

    #[test]
    fn exe_under_is_case_insensitive_and_needs_a_real_child() {
        assert!(exe_under(Path::new(r"c:\program files\foo\bin\x.exe"), Path::new(r"C:\Program Files\Foo\")));
        assert!(!exe_under(Path::new(r"D:\Program Files\Foo\x.exe"), Path::new(r"C:\Program Files\Foo")));
        assert!(!exe_under(Path::new(r"C:\Program Files\Foo"), Path::new(r"C:\Program Files\Foo")));
        assert!(!exe_under(Path::new(r"\X.EXE"), Path::new(r"C:\")));
        assert!(!exe_under(Path::new(r"\\server\share\x.exe"), Path::new(r"C:\server\share")));
    }

    #[test]
    fn expand_env_known_and_unknown() {
        let pf = std::env::var("ProgramFiles").unwrap();
        assert_eq!(expand_env(r"%ProgramFiles%\Foo"), format!(r"{pf}\Foo"));
        assert_eq!(expand_env(r"%NO_SUCH_VAR_XYZ%\Foo"), r"%NO_SUCH_VAR_XYZ%\Foo");
        assert_eq!(expand_env("50% done"), "50% done");
        assert_eq!(expand_env("plain"), "plain");
    }

    #[test]
    fn expand_env_wow64_names_the_x86_folders() {
        let x86 = std::env::var("ProgramFiles(x86)").unwrap();
        let common_x86 = std::env::var("CommonProgramFiles(x86)").unwrap();
        assert_eq!(expand_env_wow64(r"%programfiles%\Foo\a.exe"), format!(r"{x86}\Foo\a.exe"));
        assert_eq!(expand_env_wow64(r"%CommonProgramFiles%\Bar"), format!(r"{common_x86}\Bar"));
        assert_eq!(wow64_vars(r"%ProgramFiles(x86)%\X"), r"%ProgramFiles(x86)%\X");
        assert_eq!(wow64_vars("plain ä %ProgramData%"), "plain ä %ProgramData%");
    }

    #[test]
    fn first_path_quoted_with_args() {
        assert_eq!(
            first_path_in_command(r#""D:\Program Files\X\unins000.exe" /SILENT"#),
            Some(PathBuf::from(r"D:\Program Files\X\unins000.exe"))
        );
    }

    #[test]
    fn first_path_expands_env() {
        let pf = std::env::var("ProgramFiles").unwrap();
        assert_eq!(
            first_path_in_command(r#""%ProgramFiles%\Foo\unins000.exe" /SILENT"#),
            Some(PathBuf::from(format!(r"{pf}\Foo\unins000.exe")))
        );
    }

    #[test]
    fn first_path_unquoted_with_spaces_and_args() {
        assert_eq!(
            first_path_in_command(r"C:\Program Files\Foo\un.EXE /S"),
            Some(PathBuf::from(r"C:\Program Files\Foo\un.EXE"))
        );
        assert_eq!(first_path_in_command(r"C:\x\un.exe /S"), Some(PathBuf::from(r"C:\x\un.exe")));
    }

    #[test]
    fn first_path_msiexec_and_empty() {
        assert_eq!(
            first_path_in_command("MsiExec.exe /X{ABC-123}"),
            Some(PathBuf::from("MsiExec.exe"))
        );
        assert_eq!(first_path_in_command("   "), None);
        assert_eq!(first_path_in_command(""), None);
    }

    #[test]
    fn resolve_unquoted_path_follows_the_createprocess_rule() {
        let on_disk = [r"C:\ProgramData\Vendor\Some Dir\x.sys", r"C:\Program Files\Foo\svc.exe"];
        let exists = |p: &Path| on_disk.iter().any(|d| Path::new(d) == p);
        let r = |cmd: &str| resolve_unquoted_path(cmd, &exists);
        // The longest existing prefix wins over the first token.
        assert_eq!(r(r"C:\ProgramData\Vendor\Some Dir\x.sys"), Some(PathBuf::from(r"C:\ProgramData\Vendor\Some Dir\x.sys")));
        assert_eq!(r(r"C:\Program Files\Foo\svc.exe -k run"), Some(PathBuf::from(r"C:\Program Files\Foo\svc.exe")));
        // Missing: the whole string when it ends in an extension, else the first-token rule.
        assert_eq!(r(r"C:\Gone Vendor\Sub Dir\gone.sys"), Some(PathBuf::from(r"C:\Gone Vendor\Sub Dir\gone.sys")));
        assert_eq!(r(r"C:\Gone Vendor\gone.exe -k run"), Some(PathBuf::from(r"C:\Gone Vendor\gone.exe")));
        assert_eq!(r(r"C:\Gone\tool --flag"), Some(PathBuf::from(r"C:\Gone\tool")));
        // Quoted paths are taken as they are.
        assert_eq!(r(r#""C:\Gone Vendor\q.exe" /S"#), Some(PathBuf::from(r"C:\Gone Vendor\q.exe")));
        assert_eq!(r("  "), None);
    }

    #[test]
    fn long_path_name_expands_short_names_and_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let long = tmp.path().join("Long Folder Name For Test");
        std::fs::create_dir(&long).unwrap();
        assert_eq!(norm(&long_path_name(&long)), norm(&long));
        let missing = tmp.path().join("missing");
        assert_eq!(long_path_name(&missing), missing);
        // A real 8.3 name, when the volume makes them.
        let short = short_path_name(&long);
        if norm(&short) == norm(&long) {
            eprintln!("no 8.3 names on this volume; short-name case skipped");
            return;
        }
        assert!(short.to_string_lossy().contains('~'), "{}", short.display());
        assert_eq!(norm(&long_path_name(&short)), norm(&long));
    }

    /// `GetShortPathNameW`, or `p` itself when it fails.
    fn short_path_name(p: &Path) -> PathBuf {
        use windows::core::HSTRING;
        use windows::Win32::Storage::FileSystem::GetShortPathNameW;
        let input = HSTRING::from(p.as_os_str());
        let mut buf = vec![0u16; 1024];
        // SAFETY: `input` is NUL-terminated and `buf` is a valid, writable buffer.
        let len = unsafe { GetShortPathNameW(&input, Some(&mut buf)) } as usize;
        if len == 0 || len >= buf.len() {
            return p.to_path_buf();
        }
        PathBuf::from(String::from_utf16_lossy(&buf[..len]))
    }

    #[test]
    fn wide_is_nul_terminated_utf16() {
        assert_eq!(wide("aå"), vec![0x61, 0xE5, 0]);
        assert_eq!(wide(Path::new(r"C:\")), vec![0x43, 0x3A, 0x5C, 0]);
        assert_eq!(wide(""), vec![0]);
    }

    #[test]
    fn hive_short_names() {
        use crate::model::Hive;
        assert_eq!([Hive::LocalMachine, Hive::CurrentUser, Hive::Users].map(Hive::short_name), ["HKLM", "HKCU", "HKU"]);
    }

    #[test]
    fn drive_letter_cases() {
        assert_eq!(drive_letter(Path::new(r"d:\x")), Some('D'));
        assert_eq!(drive_letter(Path::new(r"C:\x")), Some('C'));
        assert_eq!(drive_letter(Path::new(r"\\server\share")), None);
        assert_eq!(drive_letter(Path::new(r"rel\path")), None);
        assert_eq!(drive_letter(Path::new("MsiExec.exe")), None);
    }
}
