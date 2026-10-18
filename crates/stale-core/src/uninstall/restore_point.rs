//! System Restore point creation through `srclient.dll`.

use std::sync::mpsc;
use std::time::Duration;

use windows::core::{s, w, BOOL};
use windows::Win32::Foundation::{FreeLibrary, ERROR_SERVICE_DISABLED, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

use super::RestorePointStatus;

pub trait RestorePointApi: Send + Sync {
    fn create(&self, description: &str) -> RestorePointStatus;
}

const BEGIN_SYSTEM_CHANGE: u32 = 100;
const END_SYSTEM_CHANGE: u32 = 101;
const APPLICATION_UNINSTALL: u32 = 1;
const MAX_DESC_W: usize = 256;
const TIMEOUT: Duration = Duration::from_secs(120);

// SRRestorePtAPI.h declares both structs under `#pragma pack(1)`, so STATEMGRSTATUS is 12 bytes.
#[repr(C, packed)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
struct RESTOREPOINTINFOW {
    dwEventType: u32,
    dwRestorePtType: u32,
    llSequenceNumber: i64,
    szDescription: [u16; MAX_DESC_W],
}

#[repr(C, packed)]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
#[derive(Default)]
struct STATEMGRSTATUS {
    nStatus: u32,
    llSequenceNumber: i64,
}

type SrSetRestorePointW = unsafe extern "system" fn(*const RESTOREPOINTINFOW, *mut STATEMGRSTATUS) -> BOOL;

/// Real System Restore via `SRSetRestorePointW`, loaded at run time from System32 (Microsoft's
/// guidance: srclient.dll is absent on some editions). Runs on a worker thread with a 2-minute timeout.
pub struct SrClient;

impl RestorePointApi for SrClient {
    fn create(&self, description: &str) -> RestorePointStatus {
        let description = description.to_string();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("restore-point".into()).spawn(move || {
            let _ = tx.send(create_blocking(&description));
        });
        if let Err(e) = spawned {
            return RestorePointStatus::Failed(format!("thread: {e}"));
        }
        match rx.recv_timeout(TIMEOUT) {
            Ok(status) => status,
            Err(mpsc::RecvTimeoutError::Timeout) => RestorePointStatus::Failed("timeout".into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => RestorePointStatus::Failed("worker thread panicked".into()),
        }
    }
}

/// Maps `SRSetRestorePointW`'s result for `BEGIN_SYSTEM_CHANGE`. Windows enforces its 24-hour
/// frequency rule by reporting success without creating anything (sequence number 0).
fn interpret(ok: bool, n_status: u32, sequence: i64) -> RestorePointStatus {
    match (ok, sequence) {
        (false, _) if n_status == ERROR_SERVICE_DISABLED.0 => RestorePointStatus::Disabled,
        (false, _) => RestorePointStatus::Failed(format!("status {n_status}")),
        (true, 0) => RestorePointStatus::SkippedFrequency,
        (true, seq) => RestorePointStatus::Created(seq),
    }
}

fn info(event: u32, sequence: i64, description: &str) -> RESTOREPOINTINFOW {
    let mut desc = [0u16; MAX_DESC_W];
    for (d, c) in desc.iter_mut().zip(description.encode_utf16().take(MAX_DESC_W - 1)) {
        *d = c;
    }
    RESTOREPOINTINFOW {
        dwEventType: event,
        dwRestorePtType: APPLICATION_UNINSTALL,
        llSequenceNumber: sequence,
        szDescription: desc,
    }
}

/// Frees the module on drop.
struct Library(HMODULE);

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: the module was loaded by LoadLibraryExW and no function pointer from it outlives this guard.
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

fn create_blocking(description: &str) -> RestorePointStatus {
    // SAFETY: srclient.dll is loaded from System32 only; `SRSetRestorePointW` has the declared
    // signature (SRRestorePtAPI.h) and is only called while `lib` keeps the module loaded. Both
    // structs are packed exactly as the header declares and live across each call.
    unsafe {
        let lib = match LoadLibraryExW(w!("srclient.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) {
            Ok(m) => Library(m),
            Err(e) => return RestorePointStatus::Failed(format!("srclient.dll: {e}")),
        };
        let Some(proc) = GetProcAddress(lib.0, s!("SRSetRestorePointW")) else {
            return RestorePointStatus::Failed("SRSetRestorePointW not found".into());
        };
        let set: SrSetRestorePointW = std::mem::transmute::<unsafe extern "system" fn() -> isize, SrSetRestorePointW>(proc);

        let begin = info(BEGIN_SYSTEM_CHANGE, 0, description);
        let mut status = STATEMGRSTATUS::default();
        let ok = set(&begin, &mut status).as_bool();
        let result = interpret(ok, status.nStatus, status.llSequenceNumber);
        if let RestorePointStatus::Created(seq) = result {
            let end = info(END_SYSTEM_CHANGE, seq, description);
            let mut end_status = STATEMGRSTATUS::default();
            if !set(&end, &mut end_status).as_bool() {
                let n = end_status.nStatus;
                log::warn!("SRSetRestorePointW(END_SYSTEM_CHANGE, {seq}) failed: status {n}");
            }
        }
        result
    }
}

/// Returns the wrapped status for every call.
pub struct FakeRestorePoint(pub RestorePointStatus);

impl RestorePointApi for FakeRestorePoint {
    fn create(&self, _description: &str) -> RestorePointStatus {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structs_match_packed_header_layout() {
        assert_eq!(size_of::<RESTOREPOINTINFOW>(), 4 + 4 + 8 + 2 * MAX_DESC_W);
        assert_eq!(size_of::<STATEMGRSTATUS>(), 12);
    }

    #[test]
    fn restore_status_interpretation() {
        assert_eq!(interpret(false, 1058, 0), RestorePointStatus::Disabled);
        assert_eq!(interpret(false, 5, 0), RestorePointStatus::Failed("status 5".into()));
        assert_eq!(interpret(true, 0, 0), RestorePointStatus::SkippedFrequency);
        assert_eq!(interpret(true, 0, 42), RestorePointStatus::Created(42));
    }

    #[test]
    fn description_is_truncated_and_nul_terminated() {
        let long = "x".repeat(400);
        let i = info(BEGIN_SYSTEM_CHANGE, 0, &long);
        let desc = i.szDescription;
        assert_eq!(desc[MAX_DESC_W - 2], u16::from(b'x'));
        assert_eq!(desc[MAX_DESC_W - 1], 0);
    }

    #[test]
    fn fake_returns_its_status() {
        assert_eq!(FakeRestorePoint(RestorePointStatus::Disabled).create("x"), RestorePointStatus::Disabled);
    }
}
