//! Helpers shared by the unit tests.

use std::path::Path;

/// Creates the directory junction `link` → `target` (`mklink /J`, no privilege needed); false when
/// it could not be created.
pub(crate) fn junction(link: &Path, target: &Path) -> bool {
    let cmd = crate::proc::system32_tool("cmd.exe");
    crate::proc::run_hidden(&cmd, &["/c".as_ref(), "mklink".as_ref(), "/J".as_ref(), link.as_os_str(), target.as_os_str()]).is_ok()
}
