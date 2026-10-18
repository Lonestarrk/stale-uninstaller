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
pub mod pathutil;
pub mod paths;
pub mod proc;
pub mod provenance;
pub mod programs;
pub mod quarantine;
pub mod registry;
pub mod regwrite;
pub mod scan;
pub mod session;
pub mod services;
pub mod settings;
pub mod shortcuts;
pub mod size;
pub mod uninstall;
pub mod usage;

pub use model::*;

/// True when this build has the test-only `e2e-overrides` feature (`STALE_E2E_*` environment
/// overrides that drop security checks). The GUI refuses to start then, e.g. after a
/// `cargo build --all-features`.
pub const E2E_OVERRIDES: bool = cfg!(feature = "e2e-overrides");

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_semver() { assert!(super::version().split('.').count() == 3); }
}
