//! Reading and writing the small files below the data folder (settings, keep list) so that a file
//! planted by someone else is never trusted: it is opened without following links, its owner is
//! checked on the same handle the content is read from, and saves replace the file (temporary
//! file renamed over it) instead of writing into whatever object the name points at.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// FILE_READ_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE.
const READ_ACCESS: u32 = 0x1 | 0x80 | 0x2_0000 | 0x10_0000;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const FILE_SHARE_DELETE: u32 = 0x4;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// The largest settings or keep-list file read (1 MiB).
pub const MAX_TRUSTED_BYTES: u64 = 1024 * 1024;

/// Opens `path` itself for reading, never a link's target (`FILE_FLAG_OPEN_REPARSE_POINT`).
/// `share_write` lets other handles keep writing (the lock-down sweep reads a planted file that
/// way); loads deny it.
pub(crate) fn open_no_follow(path: &Path, share_write: bool) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let share = FILE_SHARE_READ | FILE_SHARE_DELETE | if share_write { FILE_SHARE_WRITE } else { 0 };
    std::fs::OpenOptions::new()
        .access_mode(READ_ACCESS)
        .share_mode(share)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// True when the opened object is a link or reparse point (or a folder): never read.
pub(crate) fn is_reparse_or_dir(file: &File) -> bool {
    use std::os::windows::fs::MetadataExt;
    file.metadata().map_or(true, |md| md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !md.is_file())
}

/// Reads at most `max + 1` bytes from `file`; `None` when reading fails.
pub(crate) fn read_capped(file: &File, max: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// Whether the object behind `file` is owned by BUILTIN\Administrators or LocalSystem, read on
/// that handle; `None` when the owner cannot be read.
pub fn owner_is_admin(file: &File) -> Option<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows::Win32::Security::{
        IsWellKnownSid, WinBuiltinAdministratorsSid, WinLocalSystemSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID,
    };
    let mut owner = PSID::default();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `file` is open with READ_CONTROL and outlives the call; `owner` points into `sd`,
    // which the call allocates and which is freed below after the last use of `owner`.
    unsafe {
        let err = GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut sd),
        );
        if err != ERROR_SUCCESS {
            return None;
        }
        let ok = !owner.is_invalid()
            && (IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool() || IsWellKnownSid(owner, WinLocalSystemSid).as_bool());
        LocalFree(Some(HLOCAL(sd.0)));
        Some(ok)
    }
}

/// What [`read_trusted`] found.
#[derive(Debug, PartialEq, Eq)]
pub enum TrustedRead {
    Missing,
    /// A link or reparse point, a folder, a file not owned by Administrators/SYSTEM, too large,
    /// or unreadable: the reason, for the log.
    Refused(String),
    Text(String),
}

/// Reads `path` (at most `max` bytes) when it is a plain file whose owner `owner_trusted` accepts,
/// judged on the handle the content is then read from. [`owner_is_admin`] is the live check.
pub fn read_trusted(path: &Path, max: u64, owner_trusted: &dyn Fn(&File) -> Option<bool>) -> TrustedRead {
    let file = match open_no_follow(path, false) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TrustedRead::Missing,
        Err(e) => return TrustedRead::Refused(format!("open: {e}")),
    };
    if is_reparse_or_dir(&file) {
        return TrustedRead::Refused("a link, reparse point or folder".into());
    }
    if owner_trusted(&file) != Some(true) {
        return TrustedRead::Refused("not owned by Administrators or SYSTEM".into());
    }
    match read_capped(&file, max) {
        Some(bytes) if bytes.len() as u64 > max => TrustedRead::Refused(format!("larger than {max} bytes")),
        Some(bytes) => TrustedRead::Text(String::from_utf8_lossy(&bytes).into_owned()),
        None => TrustedRead::Refused("read failed".into()),
    }
}

/// [`read_trusted`] parsed as JSON; `T::default()` when the file is missing or unparsable, and
/// with a warning when it is refused.
pub fn load_json<T: serde::de::DeserializeOwned + Default>(path: &Path, max: u64, owner_trusted: &dyn Fn(&File) -> Option<bool>) -> T {
    match read_trusted(path, max, owner_trusted) {
        TrustedRead::Missing => T::default(),
        TrustedRead::Refused(why) => {
            log::warn!("{} was not read ({why}); using defaults", path.display());
            T::default()
        }
        TrustedRead::Text(text) => serde_json::from_str(&text).unwrap_or_default(),
    }
}

/// `<path>.tmp-<pid>-<nanos>`, beside `path`.
fn temp_beside(path: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".tmp-{}-{nanos}", std::process::id()));
    PathBuf::from(name)
}

/// Writes `bytes` to a new file beside `path` (flushed to disk) and renames it over `path`, so
/// the name ends up on a file this process created. Refuses when `path` is a link or reparse
/// point (nothing is written through it); parent folders are created.
pub fn write_replacing(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if crate::quarantine::fsmove::is_reparse_point(path) {
        return Err(std::io::Error::other(format!("{} is a link or reparse point", path.display())));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_beside(path);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        std::io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Replaces the plain file `path` with a copy this process creates: its content is read through a
/// handle that does not follow links (writers may keep their handles), then written to a new file
/// renamed over it ([`write_replacing`]). The old file object, and every handle anyone holds on
/// it, is no longer what the name opens. Refuses a link, reparse point, folder or a file over
/// [`MAX_TRUSTED_BYTES`].
pub(crate) fn replace_with_own_copy(path: &Path) -> Result<(), String> {
    let file = open_no_follow(path, true).map_err(|e| format!("open {}: {e}", path.display()))?;
    if is_reparse_or_dir(&file) {
        return Err(format!("{} is not a plain file", path.display()));
    }
    let bytes = read_capped(&file, MAX_TRUSTED_BYTES).ok_or_else(|| format!("read {} failed", path.display()))?;
    if bytes.len() as u64 > MAX_TRUSTED_BYTES {
        return Err(format!("{} is larger than {MAX_TRUSTED_BYTES} bytes", path.display()));
    }
    drop(file);
    write_replacing(path, &bytes).map_err(|e| format!("replace {}: {e}", path.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn trust_all(_: &File) -> Option<bool> {
        Some(true)
    }

    pub(crate) fn trust_none(_: &File) -> Option<bool> {
        Some(false)
    }

    /// `mklink /J link target`.
    pub(crate) fn junction(link: &Path, target: &Path) {
        let cmd = crate::proc::system32_tool("cmd.exe");
        crate::proc::run_hidden(&cmd, &["/c".as_ref(), "mklink".as_ref(), "/J".as_ref(), link.as_os_str(), target.as_os_str()])
            .unwrap();
    }

    #[test]
    fn untrusted_owner_and_reparse_points_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(read_trusted(&path, 100, &trust_all), TrustedRead::Missing);
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(read_trusted(&path, 100, &trust_all), TrustedRead::Text("{}".into()));
        assert!(matches!(read_trusted(&path, 100, &trust_none), TrustedRead::Refused(_)));
        assert!(matches!(read_trusted(&path, 1, &trust_all), TrustedRead::Refused(_)), "over the cap");
        // A junction at the name is never followed.
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("important.json");
        junction(&link, &target);
        assert!(matches!(read_trusted(&link, 100, &trust_all), TrustedRead::Refused(_)));
        // And never written through.
        assert!(write_replacing(&link, b"{}").is_err());
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0, "nothing written into the target");
        std::fs::remove_dir(&link).unwrap();
    }

    #[test]
    fn write_replacing_puts_a_new_file_at_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.json");
        write_replacing(&path, b"one").unwrap();
        write_replacing(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "no temporary file left");
    }
}
