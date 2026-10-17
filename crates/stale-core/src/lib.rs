pub mod model;
pub mod pathutil;
pub mod programs;
pub mod registry;

pub use model::*;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_semver() { assert!(super::version().split('.').count() == 3); }
}
