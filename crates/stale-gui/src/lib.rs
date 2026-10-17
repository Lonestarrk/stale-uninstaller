//! Stale's egui front end. A library so its unit tests run in a test binary without
//! the admin manifest that `build.rs` embeds into the `stale` binary.

pub mod app;
pub mod logging;
pub mod strings;
pub mod worker;
