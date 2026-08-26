use std::path::Path;

const WINDOWS_MANIFEST: &str = "windows/app.manifest";

fn main() {
    // The manifest is embedded below for every executable this crate links, so tauri-build must not
    // add its own copy to the app binary as well.
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    tauri_build::try_build(attributes).expect("tauri build");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        embed_windows_manifest();
    }
}

/// Cargo can only scope linker flags to binaries, examples, benches, or integration tests,
/// never to a library's unit-test executable. Applying the flags package-wide covers the app
/// binary and the test executables with one manifest source.
fn embed_windows_manifest() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join(WINDOWS_MANIFEST);
    println!("cargo:rerun-if-changed={WINDOWS_MANIFEST}");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}
