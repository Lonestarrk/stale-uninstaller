//! File logging to `logs_dir()\stale.log`.

use std::fs::File;

use simplelog::{Config, LevelFilter, WriteLogger};
use stale_core::paths::logs_dir;

/// Starts logging at Info level; runs without a log if the file cannot be created.
pub fn init() {
    if let Ok(file) = File::create(logs_dir().join("stale.log")) {
        let _ = WriteLogger::init(LevelFilter::Info, Config::default(), file);
    }
}
