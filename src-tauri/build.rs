fn main() {
    // Tauri's default manifest is reproduced below as a linker input so it
    // reaches Cargo's lib-test harness too. Keep Tauri's other build-time
    // setup, but prevent it from embedding a second manifest resource.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
    )
    .expect("failed to run Tauri build helpers");

    // Cargo's lib-test harness does not carry Tauri's generated resource,
    // even though linked Tauri/Wry code imports TaskDialogIndirect (a v6
    // export). Embed the same Common Controls v6 manifest for both the test
    // harness and app binary.
    println!("cargo:rerun-if-changed=AngelBot.test.manifest.xml");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        std::env::current_dir()
            .expect("build script has a current directory")
            .join("AngelBot.test.manifest.xml")
            .display()
    );
}
