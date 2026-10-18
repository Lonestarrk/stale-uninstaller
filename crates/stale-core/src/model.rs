use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
    Users,
}

impl Hive {
    /// `HKLM`, `HKCU` or `HKU`, as `reg.exe` and Regedit's address bar write them.
    pub fn short_name(self) -> &'static str {
        match self {
            Hive::LocalMachine => "HKLM",
            Hive::CurrentUser => "HKCU",
            Hive::Users => "HKU",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegKeyPath {
    pub hive: Hive,
    /// Backslash-separated, no leading slash. Empty means the hive root.
    pub path: String,
}

/// Stable identifier of a program: `"{hive:?}\{path}"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Machine,
    CurrentUser,
    OtherUser,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub id: ProgramId,
    pub key: RegKeyPath,
    pub name: String,
    pub publisher: Option<String>,
    pub version: Option<String>,
    pub install_location: Option<PathBuf>,
    pub install_date: Option<chrono::NaiveDate>,
    pub uninstall_string: Option<String>,
    pub quiet_uninstall_string: Option<String>,
    pub display_icon: Option<PathBuf>,
    pub is_msi: bool,
    pub estimated_size_bytes: Option<u64>,
    /// Uppercase drive letter.
    pub drive: Option<char>,
    pub hidden: bool,
    pub scope: Scope,
    /// Account (DOMAIN, backslash, name) owning the hive of a `Scope::OtherUser` program, when resolved.
    #[serde(default)]
    pub account: Option<String>,
}
