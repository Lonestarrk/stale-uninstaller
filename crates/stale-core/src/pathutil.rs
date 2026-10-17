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

/// Uppercase drive letter of a path; `None` for UNC or relative paths.
pub fn drive_letter(p: &Path) -> Option<char> {
    let s = p.to_str()?;
    let mut chars = s.chars();
    let letter = chars.next()?;
    (letter.is_ascii_alphabetic() && chars.next() == Some(':')).then(|| letter.to_ascii_uppercase())
}

/// Uppercase, backslash-separated, no trailing separator (`C:\` -> `C:`).
pub fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', r"\").to_uppercase();
    s.trim_end_matches('\\').to_string()
}

/// Folders (normalized) that many programs share; resolved from the environment on each call.
fn shared_folders() -> Vec<String> {
    const VARS: [&str; 10] = [
        "%ProgramFiles%",
        "%ProgramFiles(x86)%",
        "%ProgramData%",
        "%LOCALAPPDATA%",
        r"%LOCALAPPDATA%\Programs",
        "%APPDATA%",
        "%SystemRoot%",
        r"%SystemRoot%\System32",
        r"%SystemRoot%\SysWOW64",
        "%USERPROFILE%",
    ];
    VARS.iter()
        .map(|v| expand_env(v))
        .filter(|s| !s.contains('%'))
        .map(|s| norm(Path::new(&s)))
        .collect()
}

/// `%SystemDrive%\Users`, normalized.
fn users_root() -> Option<String> {
    let drive = std::env::var("SystemDrive").ok()?;
    Some(norm(Path::new(&format!(r"{drive}\Users"))))
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
    fn drive_letter_cases() {
        assert_eq!(drive_letter(Path::new(r"d:\x")), Some('D'));
        assert_eq!(drive_letter(Path::new(r"C:\x")), Some('C'));
        assert_eq!(drive_letter(Path::new(r"\\server\share")), None);
        assert_eq!(drive_letter(Path::new(r"rel\path")), None);
        assert_eq!(drive_letter(Path::new("MsiExec.exe")), None);
    }
}
