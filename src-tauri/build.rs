use std::process::Command;

fn main() {
    // The prebuilt ONNX Runtime used for vocal separation includes the
    // CoreML execution provider: it needs Apple's frameworks, plus clang's
    // runtime for `@available` checks (`___isPlatformVersionAtLeast`), which
    // rustc does not link on its own.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=CoreML");
        println!("cargo:rustc-link-lib=framework=Foundation");
        let runtime_dir = Command::new("clang")
            .arg("--print-runtime-dir")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        if let Some(dir) = runtime_dir {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
    }
    tauri_build::build()
}
