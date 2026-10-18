//! Embeds the Windows resources for StaleUninstaller.exe: the application
//! manifest (requireAdministrator, per-monitor DPI, Common Controls 6), the
//! icon (`stale.ico`, made by `assets/make-icon.ps1`) and the version info that
//! UAC and Task Manager show as the program name.

fn main() {
    // Target check (not `cfg!`, which reflects the build-script host).
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_manifest_file("StaleUninstaller.exe.manifest")
            .set_icon("stale.ico")
            .set("ProductName", "Stale Uninstaller")
            .set("FileDescription", "Stale Uninstaller")
            .set("InternalName", "StaleUninstaller")
            .set("OriginalFilename", "StaleUninstaller.exe");
        // Only the bin gets the resources; the lib's test harness must stay
        // unelevated (see Cargo.toml).
        res.compile_for("StaleUninstaller").expect("unable to compile Windows resources");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=StaleUninstaller.exe.manifest");
    println!("cargo:rerun-if-changed=stale.ico");
}
