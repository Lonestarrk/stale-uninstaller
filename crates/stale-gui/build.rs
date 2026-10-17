use embed_manifest::manifest::{DpiAwareness, ExecutionLevel};
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    // Target check (not `cfg!`, which reflects the build-script host).
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(
            new_manifest("Stale")
                .requested_execution_level(ExecutionLevel::RequireAdministrator)
                .dpi_awareness(DpiAwareness::PerMonitorV2),
        )
        .expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
