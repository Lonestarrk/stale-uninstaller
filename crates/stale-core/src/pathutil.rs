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

#[cfg(test)]
mod tests {
    use super::*;

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
