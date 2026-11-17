use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=WALLPAPER_DASHBOARD_RELEASE_TAG");
    println!("cargo:rerun-if-changed=dashboard.manifest");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest =
            PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"))
                .join("dashboard.manifest");
        println!("cargo:rustc-link-arg-bin=wallpaper-dashboard=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=wallpaper-dashboard=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
