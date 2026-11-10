//! Finds the Anzen app's images for Trezor, which the phone uploads to the device when it
//! connects, so `src/trezor.rs` can carry them inside this library.
//!
//! The images are built separately (`trezor-app/tools/build.sh`), with Trezor's nightly toolchain,
//! so they may be absent: the library then builds without a Trezor image, and says so when asked
//! to connect to one.

use std::{
    env,
    path::{Path, PathBuf},
};

/// The files of one image: the app, its proof, and its ring's root packet.
fn image(serialized: &Path, architecture: &str) -> Option<[PathBuf; 3]> {
    let versions = serialized.join("anzen.app");
    let tapp = std::fs::read_dir(&versions)
        .ok()?
        .filter_map(|version| std::fs::read_dir(version.ok()?.path()).ok())
        .flatten()
        .filter_map(|file| Some(file.ok()?.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "tapp")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains(architecture))
        })
        .max()?;
    let proof = tapp.with_extension("proof");
    // The app is in ring 2, which shares a root packet with ring 1.
    let root_packet = serialized.join("root-packets/rootpacket_ring12.tmr");
    (proof.is_file() && root_packet.is_file()).then_some([tapp, proof, root_packet])
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(trezor_device_image)");
    println!("cargo:rustc-check-cfg=cfg(trezor_emulator_image)");
    let serialized = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"))
        .join("../../trezor-app/target/artifacts/serialized");
    println!("cargo:rerun-if-changed={}", serialized.display());
    println!(
        "cargo:rerun-if-changed={}",
        serialized.join("root-packets").display()
    );

    // The Safe 7's image, for a real device over Bluetooth.
    if let Some(files) = image(&serialized, "armv8m") {
        emit("trezor_device_image", "ANZEN_TREZOR_DEVICE", &files);
    }
    // The emulator's image only goes where the emulator can be reached: a Mac or Linux host (the
    // iOS simulator runs on one, and shares its network).
    let target = env::var("TARGET").unwrap_or_default();
    let emulator = if target.contains("apple") && target.starts_with("aarch64") {
        (target.ends_with("-sim") || target.contains("darwin")).then_some("macos-aarch64")
    } else if target.contains("linux") && target.starts_with("x86_64") {
        Some("linux-x86_64")
    } else {
        None
    };
    if let Some(files) = emulator.and_then(|architecture| image(&serialized, architecture)) {
        emit("trezor_emulator_image", "ANZEN_TREZOR_EMULATOR", &files);
    }
}

fn emit(cfg: &str, variable: &str, files: &[PathBuf; 3]) {
    println!("cargo:rustc-cfg={cfg}");
    for (part, file) in ["TAPP", "PROOF", "ROOT_PACKET"].iter().zip(files) {
        let file = file.canonicalize().expect("the file exists");
        println!("cargo:rerun-if-changed={}", file.display());
        println!("cargo:rustc-env={variable}_{part}={}", file.display());
    }
}
