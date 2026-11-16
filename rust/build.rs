use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rerun-if-changed=native/macos_bridge.h");
        println!("cargo:rerun-if-changed=native/macos_bridge.m");

        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
        let obj_path = out_dir.join("macos_bridge.o");
        let lib_path = out_dir.join("libapexuse_macos.a");

        let status = Command::new("clang")
            .args([
                "-fobjc-arc",
                "-O2",
                "-mmacosx-version-min=14.0",
                "-Wall",
                "-Wextra",
                "-c",
                "native/macos_bridge.m",
                "-o",
            ])
            .arg(&obj_path)
            .status()
            .expect("failed to invoke clang");
        assert!(status.success(), "clang failed compiling native/macos_bridge.m");

        let ar_status = Command::new("ar")
            .args(["rcs"])
            .arg(&lib_path)
            .arg(&obj_path)
            .status()
            .expect("failed to invoke ar");
        assert!(ar_status.success(), "ar failed creating libapexuse_macos.a");

        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=apexuse_macos");
        println!("cargo:rustc-link-arg=-mmacosx-version-min=14.0");
        for fw in [
            "Foundation",
            "AppKit",
            "ApplicationServices",
            "CoreGraphics",
            "ImageIO",
            "Vision",
            "ScreenCaptureKit",
        ] {
            println!("cargo:rustc-link-lib=framework={fw}");
        }
    }
}
