use std::process::Command;

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    // ONNX Runtime publishes no prebuilt binaries for Intel Macs, so AI
    // separation is compiled everywhere else (see Cargo.toml).
    println!("cargo::rustc-check-cfg=cfg(has_ort)");
    let has_ort = !(os == "macos" && arch == "x86_64");
    if has_ort {
        println!("cargo:rustc-cfg=has_ort");
    }

    // On macOS the runtime includes the CoreML execution provider: it needs
    // Apple's frameworks, plus clang's runtime for `@available` checks
    // (`___isPlatformVersionAtLeast`), which rustc does not link on its own.
    if os == "macos" && has_ort {
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
