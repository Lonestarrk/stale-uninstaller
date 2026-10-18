//! Running Windows console tools (`reg.exe`, `icacls.exe`) by full path and without a window.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `%SystemRoot%\System32\<name>` (fallback `C:\Windows`), so a same-named exe next to ours is never run.
pub fn system32_tool(name: &str) -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
    PathBuf::from(root).join("System32").join(name)
}

/// Runs `program` without a window; spawn failure or a non-zero exit becomes `Err(<its output>)`.
pub fn run_hidden(program: &Path, args: &[&OsStr]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    if out.status.success() {
        return Ok(());
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    let msg = [text(&out.stderr), text(&out.stdout)].into_iter().find(|s| !s.is_empty());
    Err(msg.unwrap_or_else(|| format!("{} exited with {}", program.display(), out.status)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system32_tool_is_absolute_under_system_root() {
        let p = system32_tool("reg.exe");
        assert!(p.is_absolute(), "{}", p.display());
        assert!(p.ends_with(r"System32\reg.exe"), "{}", p.display());
    }

    #[test]
    fn run_hidden_reports_failure_output() {
        let cmd = system32_tool("cmd.exe");
        assert_eq!(run_hidden(&cmd, &["/c".as_ref(), "exit 0".as_ref()]), Ok(()));
        let err = run_hidden(&cmd, &["/c".as_ref(), "echo boom 1>&2 & exit 3".as_ref()]).unwrap_err();
        assert_eq!(err, "boom");
        assert!(run_hidden(Path::new(r"C:\definitely\missing.exe"), &[]).is_err());
    }
}
