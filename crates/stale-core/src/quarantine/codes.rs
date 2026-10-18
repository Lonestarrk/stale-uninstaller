//! Error codes in a quarantine `Failure::error` or a purge error: `"<code>"` or
//! `"<code>: <detail>"`, where the detail is a path or an OS message. The GUI words each code in
//! the user's language; anything else is shown as it is.

/// A payload `<id>` folder still holds files no manifest entry accounts for after a restore or
/// purge; the folder and the manifest are kept.
pub const UNKNOWN_CONTENT_LEFT: &str = "unknown-content-left";

/// Prefix of the purge error `"drive-missing: X:"`: a drive holding stored items is not connected,
/// so nothing is purged and the manifest is kept.
pub const DRIVE_MISSING: &str = "drive-missing";

/// Something a restore would overwrite: the original path, key, value or service exists again
/// (e.g. the program was reinstalled). The item stays in quarantine.
pub const OCCUPIED: &str = "occupied";

/// A restored item could not re-inherit its parent's ACL; it counts as restored.
pub const ACL_NOT_REINHERITED: &str = "acl-not-reinherited";

/// A folder above a restore destination is a link.
pub const REPARSE_IN_DESTINATION: &str = "reparse-in-destination";

/// A folder on the way to the stored copy is a link.
pub const REPARSE_IN_QUARANTINE_PATH: &str = "reparse-in-quarantine-path";

/// Everything was copied, but the source could not be fully removed.
pub const PARTIAL_MOVE: &str = "partial-move";

/// The manifest could not be written, so nothing was touched.
pub const MANIFEST_SAVE_FAILED: &str = "manifest-save-failed";

/// The path has no place in the quarantine folder (no drive letter).
pub const NOT_STORABLE: &str = "not-storable";

/// A manifest names a stored copy outside its quarantine folder.
pub const STORED_OUTSIDE: &str = "stored-outside-quarantine";

/// A manifest names a backup file outside its quarantine folder.
pub const BACKUP_OUTSIDE: &str = "backup-outside-quarantine";

/// A backup file is a link.
pub const BACKUP_IS_LINK: &str = "backup-is-link";

/// A quarantine folder is not owned by Administrators or SYSTEM.
pub const NOT_TRUSTED: &str = "quarantine-not-trusted";

/// A manifest's id is not a valid quarantine id.
pub const INVALID_ID: &str = "invalid-id";

/// A path that must be a real folder is a link or reparse point.
pub const IS_REPARSE: &str = "is-reparse";

/// A move's destination already exists; nothing was touched.
pub const DESTINATION_EXISTS: &str = "destination-exists";

/// A file is in use; nothing was moved.
pub const LOCKED: &str = "locked";

/// A cross-volume move found a link in the folder tree; nothing was moved.
pub const LINK_IN_TREE: &str = "link-in-tree";
