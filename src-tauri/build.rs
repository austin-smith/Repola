use std::path::Path;

const WINDOWS_MANIFEST: &str = "windows/app.manifest";

fn main() {
    let attributes = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("windows/app.manifest")),
    );
    tauri_build::try_build(attributes).expect("tauri build");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_manifest_in_test_binaries();
    }
}

/// tauri-build embeds the manifest into the app binaries only. Test executables link the
/// same dependencies, so they need the same manifest or they cannot even start on Windows.
fn embed_manifest_in_test_binaries() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join(WINDOWS_MANIFEST);
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let resource = Path::new(&out_dir).join("tests.rc");
    let manifest_path = manifest
        .to_str()
        .expect("UTF-8 manifest path")
        .replace('\\', "\\\\");
    std::fs::write(&resource, format!("1 24 \"{manifest_path}\"\n")).expect("write tests.rc");
    println!("cargo:rerun-if-changed={WINDOWS_MANIFEST}");
    embed_resource::compile_for_tests(&resource, embed_resource::NONE)
        .manifest_required()
        .expect("embed the Windows manifest into test binaries");
}
