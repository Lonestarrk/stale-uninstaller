//! Whose account the process runs as, versus who is logged in at the keyboard.
//!
//! When a standard user enters an administrator's credentials at the UAC prompt, the elevated
//! process's token (and so `HKCU` and `%APPDATA%`) belongs to the administrator, not to the
//! interactive session's user.

use windows::core::PWSTR;
use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameSamCompatible};
use windows::Win32::System::RemoteDesktop::{
    WTSDomainName, WTSFreeMemory, WTSQuerySessionInformationW, WTSUserName, WTS_CURRENT_SERVER_HANDLE,
    WTS_CURRENT_SESSION, WTS_INFO_CLASS,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionIdentity {
    /// `DOMAIN\name` of the process token (empty when it could not be read).
    pub token_user: String,
    /// `DOMAIN\name` of the interactive session's user; `None` when it could not be read.
    pub session_user: Option<String>,
}

pub fn identity() -> SessionIdentity {
    SessionIdentity { token_user: token_user().unwrap_or_default(), session_user: session_user() }
}

/// True when the interactive session's user is known and is not the token's user.
pub fn runs_as_other_account(id: &SessionIdentity) -> bool {
    let Some(session) = &id.session_user else { return false };
    let bare = |s: &str| s.rsplit('\\').next().unwrap_or(s).to_lowercase();
    !session.eq_ignore_ascii_case(&id.token_user) && bare(session) != bare(&id.token_user)
}

fn token_user() -> Option<String> {
    let mut len = 0u32;
    // SAFETY: a null buffer with a zero size only asks for the required length (in `len`).
    unsafe {
        let _ = GetUserNameExW(NameSamCompatible, None, &mut len);
    }
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` holds `len` UTF-16 units and `len` is its size; the call writes at most that many.
    let ok = unsafe { GetUserNameExW(NameSamCompatible, Some(PWSTR(buf.as_mut_ptr())), &mut len) };
    // On success `len` is the length without the terminating nul.
    ok.then(|| String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]))
}

/// One string of the current session, copied out of the WTS buffer which is then freed.
fn wts_string(class: WTS_INFO_CLASS) -> Option<String> {
    let mut ptr = PWSTR::null();
    let mut bytes = 0u32;
    // SAFETY: on success `ptr` points at a WTS-allocated nul-terminated UTF-16 string of `bytes`
    // bytes; it is read within that size and freed with WTSFreeMemory exactly once.
    unsafe {
        WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), WTS_CURRENT_SESSION, class, &mut ptr, &mut bytes)
            .ok()?;
        let units = bytes as usize / 2;
        let slice = std::slice::from_raw_parts(ptr.0, units);
        let end = slice.iter().position(|&c| c == 0).unwrap_or(units);
        let s = String::from_utf16_lossy(&slice[..end]);
        WTSFreeMemory(ptr.0.cast());
        Some(s)
    }
}

fn session_user() -> Option<String> {
    let name = wts_string(WTSUserName).filter(|n| !n.is_empty())?;
    let domain = wts_string(WTSDomainName).unwrap_or_default();
    Some(if domain.is_empty() { name } else { format!(r"{domain}\{name}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(token: &str, session: Option<&str>) -> SessionIdentity {
        SessionIdentity { token_user: token.into(), session_user: session.map(String::from) }
    }

    #[test]
    fn runs_as_other_account_compares_case_insensitively() {
        assert!(!runs_as_other_account(&id(r"PC\Jimmy", Some(r"pc\jimmy"))));
        assert!(!runs_as_other_account(&id(r"PC\jimmy", Some("jimmy"))));
        assert!(!runs_as_other_account(&id(r"PC\jimmy", None)));
        assert!(runs_as_other_account(&id(r"PC\admin", Some(r"PC\jimmy"))));
    }

    #[test]
    #[ignore = "reads the real session"]
    fn identity_live_has_token_user() {
        let id = identity();
        println!("{id:?}");
        assert!(id.token_user.contains('\\'), "{id:?}");
    }
}
