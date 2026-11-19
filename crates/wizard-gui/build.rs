fn main() {
    let mut attributes = tauri_build::Attributes::new();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        // Tauri's resource manifest is linked to binaries only. Library test executables
        // also need Common Controls v6 once IPC tests retain Tauri's native menu code,
        // otherwise the Windows loader exits with STATUS_ENTRYPOINT_NOT_FOUND.
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo package directory"),
        )
        .join("windows/app.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // Embed the same manifest through the linker for every executable, without
        // a second manifest in the application's icon/version resources.
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    }
    tauri_build::try_build(attributes).expect("failed to build Tauri resources");
}
