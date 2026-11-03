mod frontend_resource_guard;

fn main() {
    println!("cargo:rerun-if-changed=frontend_resource_guard.rs");
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    let app_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("app directory"));
    let target = tauri_utils::platform::Target::from_triple(&std::env::var("TARGET").expect("build target"));
    let overlay = std::env::var("TAURI_CONFIG").ok();
    let directory = frontend_resource_guard::validate_resources(
        &app_dir, target, overlay.as_deref(), !tauri_build::is_dev(),
    ).unwrap_or_else(|error| panic!("frontend resource contract failed: {error}"));
    println!("cargo:rerun-if-changed={}", directory.display());
    println!("cargo:warning=EXV frontend resources verified: {}", directory.display());
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // 内嵌 WebView 使 lib 单元测试也链接原生窗口实现。Tauri 的资源清单
        // 默认只链接应用二进制；统一让 MSVC 嵌入同一 Common Controls v6
        // 依赖，覆盖测试 EXE，同时避免应用二进制出现重复 MANIFEST 资源。
        let windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
            .expect("build Tauri resources");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(r#"cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"#);
    } else {
        tauri_build::build();
    }
}
