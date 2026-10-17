use std::path::PathBuf;

use windows::core::GUID;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};

/// Resolves a known-folder GUID string such as `{6D809377-...}` to its path.
/// Unknown or malformed GUIDs yield `None`.
pub fn resolve(guid: &str) -> Option<PathBuf> {
    let inner = guid.strip_prefix('{')?.strip_suffix('}')?;
    let guid = GUID::try_from(inner).ok()?;
    // SAFETY: `guid` outlives the call; the returned buffer is freed exactly once below.
    unsafe {
        let pwstr = SHGetKnownFolderPath(&guid, KF_FLAG_DEFAULT, None).ok()?;
        let s = pwstr.to_string();
        CoTaskMemFree(Some(pwstr.0 as *const _));
        s.ok().map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_malformed_guids_are_none() {
        assert!(resolve("{00000000-0000-0000-0000-000000000000}").is_none());
        assert!(resolve("not a guid").is_none());
    }

    #[test]
    fn program_files_resolves() {
        let p = resolve("{6D809377-6AF0-444B-8957-A3773F02200E}").unwrap();
        assert!(p.to_string_lossy().contains("Program Files"));
    }
}
