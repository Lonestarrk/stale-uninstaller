use crate::model::{Hive, RegKeyPath};

pub mod fake;
pub mod win;

pub use fake::FakeRegistry;
pub use win::WinRegistry;

#[derive(Clone, Debug, PartialEq)]
pub enum RegValue {
    Str(String),
    ExpandStr(String),
    MultiStr(Vec<String>),
    Dword(u32),
    Qword(u64),
    Binary(Vec<u8>),
    Other,
}

#[derive(Debug, thiserror::Error)]
pub enum RegError {
    #[error("key not found")]
    NotFound,
    #[error("access denied")]
    AccessDenied,
    #[error("{0}")]
    Other(String),
}

/// Read-only registry access. An empty `path` refers to the hive root.
pub trait RegistryReader: Send + Sync {
    fn subkeys(&self, key: &RegKeyPath) -> Result<Vec<String>, RegError>;
    fn values(&self, key: &RegKeyPath) -> Result<Vec<(String, RegValue)>, RegError>;
    fn key_exists(&self, key: &RegKeyPath) -> bool;
}

/// Convenience constructor.
pub fn key(hive: Hive, path: &str) -> RegKeyPath {
    RegKeyPath { hive, path: path.to_string() }
}
