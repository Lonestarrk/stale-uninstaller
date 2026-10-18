//! Running a program's own uninstaller and deciding whether it actually uninstalled.

use std::iter::once;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, QueryInformationJobObject,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
};
use windows::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, ResumeThread, TerminateProcess, WaitForSingleObject, CREATE_SUSPENDED,
    INFINITE, PROCESS_INFORMATION, STARTUPINFOW,
};

use super::VendorResult;
use crate::fsys::FsReader;
use crate::model::Program;
use crate::pathutil::expand_env;
use crate::proc::system32_tool;
use crate::registry::RegistryReader;

const POLL: Duration = Duration::from_millis(250);

/// `{8-4-4-4-12 hex}`, the shape of an MSI product code.
fn is_product_code(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 38
        && b[0] == b'{'
        && b[37] == b'}'
        && b[1..37].iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

fn msiexec() -> String {
    system32_tool("msiexec.exe").to_string_lossy().into_owned()
}

fn is_msiexec(exe: &str) -> bool {
    let name = exe.rsplit(['\\', '/']).next().unwrap_or("");
    name.eq_ignore_ascii_case("msiexec.exe") || name.eq_ignore_ascii_case("msiexec")
}

/// Splits a command line into (exe, rest): a quoted first token, else the first token, else (for an
/// unquoted path with spaces) everything up to the first `.exe`.
fn split_exe(cmd: &str) -> (&str, &str) {
    if let Some(after) = cmd.strip_prefix('"') {
        return match after.find('"') {
            Some(end) => (&after[..end], &after[end + 1..]),
            None => (after, ""),
        };
    }
    let token_end = cmd.find(char::is_whitespace).unwrap_or(cmd.len());
    let token = &cmd[..token_end];
    let exe_end = match cmd.to_ascii_lowercase().find(".exe") {
        // Only for a path-looking first token, so `msiexec /x foo.exe` keeps `msiexec` as the exe.
        Some(i) if token.contains('\\') && !token.to_ascii_lowercase().ends_with(".exe") => i + 4,
        _ => token_end,
    };
    (&cmd[..exe_end], &cmd[exe_end..])
}

/// Whitespace-separated arguments; a quoted section (quotes kept) never splits its token.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        if c == '"' {
            quoted = !quoted;
        }
        if c.is_whitespace() && !quoted {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The command to run `p`'s own uninstaller (exe first), or `None` if it has none we can run.
/// `QuietUninstallString` is never used: the vendor uninstaller runs interactively.
pub fn uninstall_command(p: &Program, exists: &dyn Fn(&Path) -> bool) -> Option<Vec<String>> {
    let leaf = p.key.path.rsplit('\\').next().unwrap_or("");
    if p.is_msi && is_product_code(leaf) {
        return Some(vec![msiexec(), "/x".to_string(), leaf.to_string()]);
    }
    let raw = p.uninstall_string.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    let (exe, rest) = split_exe(raw);
    let exe = expand_env(exe.trim());
    let args = split_args(rest);
    if is_msiexec(&exe) {
        let removes = args.iter().any(|a| {
            let a = a.to_ascii_lowercase();
            ["/x", "/i", "-x", "-i"].iter().any(|s| a.starts_with(s))
        });
        // Always System32's msiexec, whatever path the registry names.
        return removes.then(|| once(msiexec()).chain(args).collect());
    }
    if exe.is_empty() {
        return None;
    }
    // A bare name (`rundll32.exe`, `RunDll32`) would go through CreateProcessW's search order, which
    // starts with our own folder and the current directory: pin it to System32. A relative path
    // with separators has the same problem and no safe base, so it is refused.
    let exe = if !exe.contains(['\\', '/']) {
        let name = if exe.to_ascii_lowercase().ends_with(".exe") { exe } else { format!("{exe}.exe") };
        system32_tool(&name).to_string_lossy().into_owned()
    } else if Path::new(&exe).is_absolute() {
        exe
    } else {
        return None;
    };
    if !exists(Path::new(&exe)) {
        return None;
    }
    Some(once(exe).chain(args).collect())
}

/// Builds a `CreateProcessW` command line: the exe always quoted; an argument quoted only if it has
/// whitespace and no quotes of its own (vendor arguments like `/D="C:\x y"` pass through verbatim).
fn command_line(cmd: &[String]) -> String {
    let quote = |s: &str| {
        // Backslashes before the closing quote must be doubled or they escape it.
        let trailing = s.len() - s.trim_end_matches('\\').len();
        format!("\"{s}{}\"", "\\".repeat(trailing))
    };
    let mut parts = Vec::with_capacity(cmd.len());
    for (i, a) in cmd.iter().enumerate() {
        let needs = i == 0 || a.is_empty() || (a.contains(char::is_whitespace) && !a.contains('"'));
        parts.push(if needs { quote(a) } else { a.clone() });
    }
    parts.join(" ")
}

pub trait ProcessRunner: Send + Sync {
    /// Runs `cmd` (exe first) and returns the root process's exit code once every process it started has exited.
    fn run_and_wait_tree(&self, cmd: &[String]) -> Result<i32, String>;
}

/// Runs the command in a Job Object and waits until every process in the job has exited, so
/// uninstallers that hand off to a temp copy of themselves (Inno Setup, NSIS) are waited for.
/// The job has no kill-on-close: an uninstaller keeps running if we crash, and nothing is ever killed.
///
/// There is no time limit: the wait ends when the job is empty, or, once the root process has
/// exited, when `stop_waiting` is set (the user chose to move on although children linger, e.g. a
/// browser the uninstaller opened); the root's exit code is returned. While the root is still
/// running the wait continues regardless of the flag.
pub struct JobObjectRunner {
    stop_waiting: Arc<AtomicBool>,
}

impl JobObjectRunner {
    pub fn new(stop_waiting: Arc<AtomicBool>) -> Self {
        JobObjectRunner { stop_waiting }
    }
}

impl Default for JobObjectRunner {
    fn default() -> Self {
        JobObjectRunner::new(Arc::new(AtomicBool::new(false)))
    }
}

/// Closes the handle on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was returned open by a Win32 call and is owned only by this guard.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Active process count of the job, or `None` if it cannot be queried.
fn active_processes(job: &Owned) -> Option<u32> {
    let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
    // SAFETY: `job` is an open job handle and `info` is a writable buffer of the size passed.
    let query = unsafe {
        QueryInformationJobObject(
            Some(job.0),
            JobObjectBasicAccountingInformation,
            &mut info as *mut _ as *mut core::ffi::c_void,
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
            None,
        )
    };
    match query {
        Ok(()) => Some(info.ActiveProcesses),
        Err(e) => {
            log::warn!("QueryInformationJobObject: {e}; waiting for the root process only");
            None
        }
    }
}

fn root_exited(process: &Owned) -> bool {
    // SAFETY: `process` is an open process handle; a zero timeout only polls its state.
    unsafe { WaitForSingleObject(process.0, 0) == WAIT_OBJECT_0 }
}

impl JobObjectRunner {
    /// Polls every [`POLL`] until the job is empty (without a job: until the root has exited), or
    /// the root has exited and `stop_waiting` is set.
    fn wait(&self, mut job: Option<&Owned>, process: &Owned) {
        loop {
            let exited = root_exited(process);
            if exited && self.stop_waiting.load(Ordering::Relaxed) {
                return;
            }
            match job {
                Some(j) => match active_processes(j) {
                    Some(0) => return,
                    Some(_) => {}
                    None => job = None,
                },
                None if exited => return,
                None => {}
            }
            std::thread::sleep(POLL);
        }
    }
}

impl ProcessRunner for JobObjectRunner {
    fn run_and_wait_tree(&self, cmd: &[String]) -> Result<i32, String> {
        if cmd.is_empty() {
            return Err("empty command".into());
        }
        let mut line: Vec<u16> = command_line(cmd).encode_utf16().chain(once(0)).collect();
        // SAFETY: `line` is a NUL-terminated, writable UTF-16 buffer that outlives CreateProcessW;
        // `si`/`pi` are valid for the call. Every returned handle is wrapped in `Owned` at once, so
        // it is closed on every path out of this block.
        unsafe {
            let job = Owned(CreateJobObjectW(None, PCWSTR::null()).map_err(|e| format!("CreateJobObjectW: {e}"))?);
            let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
            let mut pi = PROCESS_INFORMATION::default();
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_SUSPENDED,
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            )
            .map_err(|e| format!("{}: {e}", cmd[0]))?;
            let process = Owned(pi.hProcess);
            let thread = Owned(pi.hThread);
            let in_job = match AssignProcessToJobObject(job.0, process.0) {
                Ok(()) => true,
                Err(e) if e.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0) => {
                    log::warn!("{}: cannot join a job ({e}); waiting for the root process only", cmd[0]);
                    false
                }
                Err(e) => {
                    let _ = TerminateProcess(process.0, 1);
                    return Err(format!("AssignProcessToJobObject: {e}"));
                }
            };
            if ResumeThread(thread.0) == u32::MAX {
                let e = windows::core::Error::from_thread();
                let _ = TerminateProcess(process.0, 1);
                return Err(format!("ResumeThread: {e}"));
            }
            drop(thread);
            self.wait(in_job.then_some(&job), &process);
            let waited = WaitForSingleObject(process.0, INFINITE);
            if waited != WAIT_OBJECT_0 {
                return Err(format!("WaitForSingleObject: {waited:?} ({})", windows::core::Error::from_thread()));
            }
            let mut code = 0u32;
            GetExitCodeProcess(process.0, &mut code).map_err(|e| format!("GetExitCodeProcess: {e}"))?;
            // Exit codes are DWORDs; NTSTATUS-style values like 0xC0000005 become negative.
            Ok(code as i32)
        }
    }
}

/// Records each command and returns `exit`.
pub struct FakeProcessRunner {
    pub exit: i32,
    pub calls: Mutex<Vec<Vec<String>>>,
}

impl ProcessRunner for FakeProcessRunner {
    fn run_and_wait_tree(&self, cmd: &[String]) -> Result<i32, String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(cmd.to_vec());
        Ok(self.exit)
    }
}

/// Subfolder levels below the install folder that [`still_installed`] searches for an exe.
const STILL_INSTALLED_DEPTH: usize = 4;

/// Whether `dir` or a folder up to [`STILL_INSTALLED_DEPTH`] levels below it holds a `*.exe`.
/// Links are not followed (`list_dir` reports them as non-folders).
fn has_exe(fs: &dyn FsReader, dir: &Path) -> bool {
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for e in fs.list_dir(&dir) {
            if e.is_dir {
                if depth < STILL_INSTALLED_DEPTH {
                    stack.push((e.path, depth + 1));
                }
            } else if e.path.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
                return true;
            }
        }
    }
    false
}

/// Spec §6.2: the program counts as installed while its Uninstall key exists and its install folder
/// (when it has one) still holds at least one `*.exe`, directly or up to four folder levels below
/// (e.g. `bin\x64\app.exe`).
pub fn still_installed(reg: &dyn RegistryReader, fs: &dyn FsReader, p: &Program) -> bool {
    if !reg.key_exists(&p.key) {
        return false;
    }
    match &p.install_location {
        None => true,
        Some(dir) => fs.is_dir(dir) && has_exe(fs, dir),
    }
}

/// MSI's "user cancelled" (1602) and "fatal error" (1603): the uninstall did not happen, whatever
/// the registry and the folder look like.
const MSI_NOT_UNINSTALLED: [i32; 2] = [1602, 1603];

/// Runs `p`'s own uninstaller, waits for its whole process tree, then checks whether it is gone.
pub fn run_vendor_uninstaller(
    p: &Program,
    runner: &dyn ProcessRunner,
    reg: &dyn RegistryReader,
    fs: &dyn FsReader,
) -> VendorResult {
    let Some(cmd) = uninstall_command(p, &|x| fs.exists(x)) else {
        return VendorResult::NoUninstaller;
    };
    match runner.run_and_wait_tree(&cmd) {
        Err(e) => {
            log::warn!("{}: uninstaller failed to run: {e}", p.name);
            VendorResult::NotUninstalled { exit_code: None }
        }
        Ok(code) if MSI_NOT_UNINSTALLED.contains(&code) || still_installed(reg, fs, p) => {
            VendorResult::NotUninstalled { exit_code: Some(code) }
        }
        Ok(code) => VendorResult::Uninstalled { exit_code: code },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsys::FakeFs;
    use crate::model::{Hive, ProgramId, RegKeyPath, Scope};
    use crate::proc::system32_tool;
    use crate::registry::FakeRegistry;
    use std::path::PathBuf;
    use std::time::Instant;

    const GUID: &str = "{12345678-ABCD-EF01-2345-6789ABCDEF01}";
    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

    fn program(leaf: &str, uninstall: Option<&str>, is_msi: bool, install: Option<&str>) -> Program {
        Program {
            id: ProgramId(leaf.into()),
            key: RegKeyPath { hive: Hive::LocalMachine, path: format!(r"{UNINSTALL}\{leaf}") },
            name: "Foo".into(),
            publisher: None,
            version: None,
            install_location: install.map(PathBuf::from),
            install_date: None,
            uninstall_string: uninstall.map(str::to_string),
            quiet_uninstall_string: Some(r#""C:\Foo\unins000.exe" /VERYSILENT"#.into()),
            display_icon: None,
            is_msi,
            estimated_size_bytes: None,
            drive: Some('C'),
            hidden: false,
            scope: Scope::Machine,
        }
    }

    fn msiexec() -> String {
        system32_tool("msiexec.exe").to_string_lossy().into_owned()
    }

    fn fake_runner(exit: i32) -> FakeProcessRunner {
        FakeProcessRunner { exit, calls: Mutex::new(Vec::new()) }
    }

    struct FailingRunner;
    impl ProcessRunner for FailingRunner {
        fn run_and_wait_tree(&self, _cmd: &[String]) -> Result<i32, String> {
            Err("CreateProcessW failed".into())
        }
    }

    #[test]
    fn msi_command_uses_product_code() {
        let p = program(GUID, Some(&format!("MsiExec.exe /I{GUID}")), true, None);
        let cmd = uninstall_command(&p, &|_| false).unwrap();
        assert_eq!(cmd, vec![msiexec(), "/x".to_string(), GUID.to_string()]);
    }

    #[test]
    fn exe_command_keeps_arguments() {
        let p = program("Foo_is1", Some(r#""C:\Foo\unins000.exe" /SILENT"#), false, None);
        let cmd = uninstall_command(&p, &|_| true).unwrap();
        assert_eq!(cmd, vec![r"C:\Foo\unins000.exe".to_string(), "/SILENT".to_string()]);
        // A quoted argument stays one token, quotes included.
        let p = program("Foo", Some(r#"C:\Foo\uninst.exe /D="C:\Foo Bar" /S"#), false, None);
        let cmd = uninstall_command(&p, &|_| true).unwrap();
        assert_eq!(cmd, vec![r"C:\Foo\uninst.exe", r#"/D="C:\Foo Bar""#, "/S"]);
        // Unquoted exe path containing spaces.
        let p = program("Foo", Some(r"C:\Program Files\Foo\uninst.exe /S"), false, None);
        let cmd = uninstall_command(&p, &|_| true).unwrap();
        assert_eq!(cmd, vec![r"C:\Program Files\Foo\uninst.exe", "/S"]);
    }

    #[test]
    fn missing_uninstaller_exe_is_none() {
        let p = program("Foo_is1", Some(r#""C:\Foo\unins000.exe" /SILENT"#), false, None);
        let seen = Mutex::new(Vec::new());
        let exists = |x: &Path| {
            seen.lock().unwrap().push(x.to_path_buf());
            false
        };
        assert_eq!(uninstall_command(&p, &exists), None);
        assert_eq!(*seen.lock().unwrap(), vec![PathBuf::from(r"C:\Foo\unins000.exe")]);
        // Never falls back to QuietUninstallString.
        assert_eq!(uninstall_command(&program("Foo", None, false, None), &|_| true), None);
        assert_eq!(uninstall_command(&program("Foo", Some("  "), false, None), &|_| true), None);
    }

    #[test]
    fn uninstall_command_resolves_msiexec_from_uninstall_string() {
        // MSI flag but the key leaf is not a product code: use UninstallString, msiexec from System32.
        let p = program("Foo", Some(&format!("MsiExec.exe /X{GUID}")), true, None);
        assert_eq!(uninstall_command(&p, &|_| false).unwrap(), vec![msiexec(), format!("/X{GUID}")]);
        let p = program("Foo", Some(&format!("msiexec /i {GUID}")), false, None);
        assert_eq!(uninstall_command(&p, &|_| false).unwrap(), vec![msiexec(), "/i".to_string(), GUID.to_string()]);
        // A later `.exe` argument does not get swallowed into the exe token.
        let p = program("Foo", Some(r"msiexec /x C:\setup.exe"), false, None);
        assert_eq!(uninstall_command(&p, &|_| false).unwrap(), vec![msiexec(), "/x".to_string(), r"C:\setup.exe".to_string()]);
        // msiexec without an uninstall/maintenance switch is not an uninstaller.
        let p = program("Foo", Some(r"msiexec.exe /qn /p C:\patch.msp"), false, None);
        assert_eq!(uninstall_command(&p, &|_| true), None);
    }

    #[test]
    fn command_line_quotes_exe_and_spaced_args() {
        let cmd = [r"C:\Foo\u.exe", "/S", r#"/D="C:\a b""#, r"C:\x y\", ""].map(String::from);
        assert_eq!(command_line(&cmd), r#""C:\Foo\u.exe" /S /D="C:\a b" "C:\x y\\" """#);
    }

    #[test]
    fn bare_rundll32_resolves_to_system32() {
        let rundll = system32_tool("rundll32.exe");
        let rundll = rundll.to_string_lossy().into_owned();
        // Case-insensitive like the real file system.
        let only_rundll = |x: &Path| x.to_string_lossy().eq_ignore_ascii_case(&rundll);
        for raw in ["rundll32.exe dfshim.dll,ShArpMaintain Foo.application", "RunDll32 dfshim.dll,ShArpMaintain Foo.application"] {
            let p = program("Foo", Some(raw), false, None);
            let cmd = uninstall_command(&p, &only_rundll).unwrap();
            assert!(cmd[0].eq_ignore_ascii_case(&rundll), "{raw}: {}", cmd[0]);
            assert_eq!(cmd[1..], ["dfshim.dll,ShArpMaintain", "Foo.application"], "{raw}");
        }
        let regsvr = system32_tool("regsvr32.exe");
        let p = program("Foo", Some(r"regsvr32 /u /s C:\Foo\x.dll"), false, None);
        let cmd = uninstall_command(&p, &|x| x == regsvr.as_path()).unwrap();
        assert_eq!(cmd, vec![regsvr.to_string_lossy().into_owned(), "/u".into(), "/s".into(), r"C:\Foo\x.dll".into()]);
    }

    #[test]
    fn bare_unknown_exe_is_none() {
        let seen = Mutex::new(Vec::new());
        let exists = |x: &Path| {
            seen.lock().unwrap().push(x.to_path_buf());
            false
        };
        let p = program("Foo", Some("uninst.exe /S"), false, None);
        assert_eq!(uninstall_command(&p, &exists), None);
        assert_eq!(*seen.lock().unwrap(), vec![system32_tool("uninst.exe")]);
        // A relative path with separators has no safe base.
        let p = program("Foo", Some(r"sub\uninst.exe /S"), false, None);
        assert_eq!(uninstall_command(&p, &|_| true), None);
    }

    fn installed_fs() -> FakeFs {
        FakeFs::new().dir(r"C:\Foo", 0).file(r"C:\Foo\Foo.EXE", 10).file(r"C:\Foo\readme.txt", 1)
    }

    fn registry_with(p: &Program) -> FakeRegistry {
        let mut reg = FakeRegistry::new();
        reg.add_key(p.key.clone());
        reg
    }

    #[test]
    fn still_installed_requires_key_and_exe() {
        let p = program("Foo", Some(r"C:\Foo\u.exe"), false, Some(r"C:\Foo"));
        let reg = registry_with(&p);
        assert!(still_installed(&reg, &installed_fs(), &p));
        // Key gone.
        assert!(!still_installed(&FakeRegistry::new(), &installed_fs(), &p));
        // Folder without an exe.
        let no_exe = FakeFs::new().dir(r"C:\Foo", 0).file(r"C:\Foo\readme.txt", 1);
        assert!(!still_installed(&reg, &no_exe, &p));
        // Folder gone.
        assert!(!still_installed(&reg, &FakeFs::new(), &p));
        // No install location: the key alone decides.
        let p = program("Foo", Some(r"C:\Foo\u.exe"), false, None);
        assert!(still_installed(&reg, &FakeFs::new(), &p));
    }

    #[test]
    fn vendor_result_not_uninstalled_when_key_and_exe_remain() {
        let p = program("Foo", Some(r#""C:\Foo\unins000.exe" /SILENT"#), false, Some(r"C:\Foo"));
        let fs = installed_fs().file(r"C:\Foo\unins000.exe", 5);
        let runner = fake_runner(1602);
        let r = run_vendor_uninstaller(&p, &runner, &registry_with(&p), &fs);
        assert_eq!(r, VendorResult::NotUninstalled { exit_code: Some(1602) });
        assert_eq!(*runner.calls.lock().unwrap(), vec![vec![r"C:\Foo\unins000.exe".to_string(), "/SILENT".to_string()]]);
    }

    #[test]
    fn vendor_result_uninstalled_when_key_gone() {
        let p = program("Foo", Some(r#""C:\Foo\unins000.exe" /SILENT"#), false, Some(r"C:\Foo"));
        let fs = installed_fs().file(r"C:\Foo\unins000.exe", 5);
        let r = run_vendor_uninstaller(&p, &fake_runner(0), &FakeRegistry::new(), &fs);
        assert_eq!(r, VendorResult::Uninstalled { exit_code: 0 });
        // No uninstaller on disk: nothing is run.
        let runner = fake_runner(0);
        let r = run_vendor_uninstaller(&p, &runner, &FakeRegistry::new(), &FakeFs::new());
        assert_eq!(r, VendorResult::NoUninstaller);
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn still_installed_finds_exe_in_bin_subfolder() {
        let p = program("Foo", Some(r"C:\Foo\u.exe"), false, Some(r"C:\Foo"));
        let reg = registry_with(&p);
        let fs = FakeFs::new().dir(r"C:\Foo", 0).file(r"C:\Foo\readme.txt", 1).file(r"C:\Foo\bin\x64\foo.exe", 10);
        assert!(still_installed(&reg, &fs, &p));
        let deep = FakeFs::new().dir(r"C:\Foo", 0).file(r"C:\Foo\a\b\c\d\foo.exe", 10);
        assert!(still_installed(&reg, &deep, &p), "four levels down");
        let too_deep = FakeFs::new().dir(r"C:\Foo", 0).file(r"C:\Foo\a\b\c\d\e\foo.exe", 10);
        assert!(!still_installed(&reg, &too_deep, &p), "five levels down is not searched");
    }

    #[test]
    fn msi_1602_is_not_uninstalled_even_if_key_gone() {
        let p = program(GUID, Some(&format!("MsiExec.exe /X{GUID}")), true, None);
        for code in [1602, 1603] {
            let r = run_vendor_uninstaller(&p, &fake_runner(code), &FakeRegistry::new(), &FakeFs::new());
            assert_eq!(r, VendorResult::NotUninstalled { exit_code: Some(code) });
        }
        let r = run_vendor_uninstaller(&p, &fake_runner(0), &FakeRegistry::new(), &FakeFs::new());
        assert_eq!(r, VendorResult::Uninstalled { exit_code: 0 });
    }

    #[test]
    fn runner_error_is_not_uninstalled() {
        let p = program("Foo", Some(r#""C:\Foo\unins000.exe" /SILENT"#), false, Some(r"C:\Foo"));
        let fs = installed_fs().file(r"C:\Foo\unins000.exe", 5);
        let r = run_vendor_uninstaller(&p, &FailingRunner, &FakeRegistry::new(), &fs);
        assert_eq!(r, VendorResult::NotUninstalled { exit_code: None });
    }

    #[test]
    #[ignore = "spawns real processes"]
    fn job_runner_waits_for_child_tree() {
        // The root cmd exits at once; its detached child sleeps ~3 s. `ping` rather than `timeout`:
        // timeout.exe refuses to run without console input ("Input redirection is not supported")
        // and a bare `timeout` may resolve to Git Bash's coreutils. Full paths for the same reason.
        let shell = system32_tool("cmd.exe").to_string_lossy().into_owned();
        let ping = system32_tool("ping.exe").to_string_lossy().into_owned();
        let cmd = vec![
            shell.clone(),
            "/c".to_string(),
            format!("start /b {shell} /c {ping} -n 4 127.0.0.1 >nul & exit 0"),
        ];
        let start = std::time::Instant::now();
        let code = JobObjectRunner::default().run_and_wait_tree(&cmd).unwrap();
        let elapsed = start.elapsed();
        println!("exit {code}, elapsed {elapsed:?}");
        assert_eq!(code, 0);
        assert!(elapsed >= std::time::Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    #[ignore = "spawns real processes"]
    fn job_runner_stop_flag_returns_after_root_exit() {
        // The root exits at once with 7; its detached ping child lives ~29 s in the job.
        let shell = system32_tool("cmd.exe").to_string_lossy().into_owned();
        let ping = system32_tool("ping.exe").to_string_lossy().into_owned();
        let cmd = vec![shell, "/c".to_string(), format!("start /b {ping} -n 30 127.0.0.1 >nul & exit 7")];
        let stop = Arc::new(AtomicBool::new(false));
        let setter = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                stop.store(true, Ordering::Relaxed);
            })
        };
        let start = Instant::now();
        let code = JobObjectRunner::new(stop).run_and_wait_tree(&cmd).unwrap();
        let elapsed = start.elapsed();
        setter.join().unwrap();
        println!("exit {code}, elapsed {elapsed:?}");
        assert_eq!(code, 7);
        assert!(elapsed >= Duration::from_secs(1), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }
}
