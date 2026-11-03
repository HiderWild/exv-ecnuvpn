fn main() {
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
