use std::env;

fn main() {
    if env::var("CARGO_FEATURE_TEST").is_ok() {
        return;
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=System");
        println!("cargo:rustc-link-arg=-Wl,-export_dynamic");
    } else if env::var("CARGO_CFG_UNIX").is_ok() || target_os == "linux" {
        println!("cargo:rustc-link-lib=c");
        println!("cargo:rustc-link-arg=-shared");
    }
}
