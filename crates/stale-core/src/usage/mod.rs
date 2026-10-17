use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod bam;
pub mod filetime;
pub mod matching;
pub mod prefetch;
pub mod userassist;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UsageSource {
    Prefetch,
    Bam,
    UserAssist,
    FileTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageHit {
    pub exe_path: PathBuf,
    pub last_run: DateTime<Utc>,
    pub source: UsageSource,
    pub run_count: Option<u32>,
}

pub trait UsageProvider: Send {
    fn name(&self) -> &'static str;
    fn collect(&self) -> Result<Vec<UsageHit>, String>;

    /// Like `collect`, plus warnings about partial results (shown to the user, the hits are
    /// still used). `Err` still means the provider produced nothing.
    fn collect_with_warnings(&self) -> (Result<Vec<UsageHit>, String>, Vec<String>) {
        (self.collect(), vec![])
    }
}
