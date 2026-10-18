pub mod devicepath;
pub mod fsys;
pub mod knownfolders;
pub mod leftovers;
pub mod model;
pub mod pathutil;
pub mod paths;
pub mod programs;
pub mod quarantine;
pub mod registry;
pub mod regwrite;
pub mod scan;
pub mod services;
pub mod settings;
pub mod shortcuts;
pub mod size;
pub mod usage;

pub use model::*;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_semver() { assert!(super::version().split('.').count() == 3); }
}
