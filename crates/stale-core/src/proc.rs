//! Running Windows console tools (`reg.exe`, `sc.exe`, `schtasks.exe`) by full path and without a window.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `System32\<name>`, System32 from the system ([`crate::paths::system32_dir`]), never from
/// `%SystemRoot%`, so neither a same-named exe next to ours nor a changed environment is run.
pub fn system32_tool(name: &str) -> PathBuf {
    crate::paths::system32_dir().join(name)
}

/// Console tool output as text: decoded with the OEM code page the tools write in (`GetOEMCP`,
/// e.g. 850, where `ä` is 0x84); lossy UTF-8 if that conversion fails.
pub fn decode_oem(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{GetOEMCP, MultiByteToWideChar, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
    if bytes.is_empty() || bytes.len() > i32::MAX as usize {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    // SAFETY: plain calls with valid slices; the second fills `wide`, sized by the first.
    unsafe {
        let cp = GetOEMCP();
        let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
        let len = MultiByteToWideChar(cp, flags, bytes, None);
        if len <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; len as usize];
        let written = MultiByteToWideChar(cp, flags, bytes, Some(&mut wide));
        if written <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        String::from_utf16_lossy(&wide[..written as usize])
    }
}

/// Runs `program` without a window and returns its exit code and output (stdout, then stderr);
/// only a failure to start it is an `Err`.
pub fn run_hidden_output(program: &Path, args: &[&OsStr]) -> Result<(i32, String), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    let text = format!("{}{}", decode_oem(&out.stdout), decode_oem(&out.stderr));
    Ok((out.status.code().unwrap_or(-1), text))
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
    let text = |b: &[u8]| decode_oem(b).trim().to_string();
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
        assert!(p.to_string_lossy().to_lowercase().ends_with(r"system32\reg.exe"), "{}", p.display());
    }

    #[test]
    fn system32_tool_does_not_depend_on_env() {
        let real = crate::paths::windows_dir();
        let saved = std::env::var_os("SystemRoot");
        // Edition 2021: set_var is safe; restored right after.
        std::env::set_var("SystemRoot", r"C:\Evil");
        let (tool, tasks, windows) = (system32_tool("reg.exe"), crate::paths::tasks_root(), crate::paths::windows_dir());
        match saved {
            Some(v) => std::env::set_var("SystemRoot", v),
            None => std::env::remove_var("SystemRoot"),
        }
        let under_real = |p: &Path| p.to_string_lossy().to_lowercase().starts_with(&real.to_string_lossy().to_lowercase());
        assert!(!real.to_string_lossy().to_lowercase().starts_with(r"c:\evil"), "{}", real.display());
        assert!(under_real(&tool) && tool.to_string_lossy().to_lowercase().ends_with(r"system32\reg.exe"), "{}", tool.display());
        assert!(under_real(&tasks), "{}", tasks.display());
        assert_eq!(windows, real);
    }

    #[test]
    fn decode_oem_uses_the_oem_code_page() {
        assert_eq!(decode_oem(b"plain ascii"), "plain ascii");
        assert_eq!(decode_oem(b""), "");
        // SAFETY: no arguments.
        let cp = unsafe { windows::Win32::Globalization::GetOEMCP() };
        if cp == 850 || cp == 437 {
            assert_eq!(decode_oem(b"Angiven tj\x84nst \x84r inte installerad."), "Angiven tjänst är inte installerad.");
        }
    }

    #[test]
    fn run_hidden_reports_failure_output() {
        let cmd = system32_tool("cmd.exe");
        assert_eq!(run_hidden(&cmd, &["/c".as_ref(), "exit 0".as_ref()]), Ok(()));
        let err = run_hidden(&cmd, &["/c".as_ref(), "echo boom 1>&2 & exit 3".as_ref()]).unwrap_err();
        assert_eq!(err, "boom");
        assert!(run_hidden(Path::new(r"C:\definitely\missing.exe"), &[]).is_err());
        let (code, out) = run_hidden_output(&cmd, &["/c".as_ref(), "echo hello& exit 3".as_ref()]).unwrap();
        assert_eq!((code, out.trim()), (3, "hello"));
    }
}
