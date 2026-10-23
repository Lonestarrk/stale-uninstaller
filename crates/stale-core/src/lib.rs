#[cfg(all(feature = "e2e-overrides", not(debug_assertions)))]
compile_error!("e2e-overrides is a debug-only feature");

pub mod acl;
pub mod dependencies;
pub mod devicepath;
pub mod fsys;
pub mod ghosts;
pub mod keep;
pub mod knownfolders;
pub mod leftovers;
pub mod model;
pub mod offline_hives;
pub mod pathutil;
pub mod paths;
pub mod proc;
pub mod provenance;
pub mod programs;
pub mod quarantine;
pub mod registry;
pub mod regwrite;
pub mod scan;
pub mod selfclean;
pub mod session;
pub mod services;
pub mod settings;
pub mod shortcuts;
pub mod size;
pub mod trusted;
pub mod uninstall;
pub mod usage;

pub use model::*;

/// True when this build has the test-only `e2e-overrides` feature (`STALE_E2E_*` environment
/// overrides that drop security checks). The GUI refuses to start then, e.g. after a
/// `cargo build --all-features`.
pub const E2E_OVERRIDES: bool = cfg!(feature = "e2e-overrides");

/// Serialises the tests that change `SystemRoot` (or another variable the crate reads) with the
/// tests that read `SystemRoot`/`ProgramData` through the environment.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Takes [`ENV_LOCK`]; a test that panicked while holding it does not poison the others.
#[cfg(test)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
pub(crate) mod testutil;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_semver() { assert!(super::version().split('.').count() == 3); }
}
