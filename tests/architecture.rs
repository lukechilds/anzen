use std::{
    fs,
    path::{Path, PathBuf},
};

fn source(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap()
}

fn rust_files(directory: &str) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect()
}

fn production_source(path: &Path) -> String {
    source(path)
        .split("#[cfg(test)]")
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn core_has_no_device_implementation_dependencies() {
    for path in rust_files("src/core") {
        let source = production_source(&path);
        assert!(
            !source.contains("crate::hot_wallet") && !source.contains("crate::cold_wallet"),
            "{} crosses the core dependency boundary",
            path.display()
        );
    }
}

#[test]
fn hot_and_cold_wallets_are_independent() {
    for path in rust_files("src/hot_wallet") {
        assert!(
            !source(&path).contains("crate::cold_wallet"),
            "{} imports the cold wallet",
            path.display()
        );
    }
    for path in rust_files("src/cold_wallet") {
        let source = source(&path);
        assert!(
            !source.lines().any(|line| {
                !line.trim_start().starts_with("//!") && line.contains("crate::hot_wallet")
            }),
            "{} imports the hot wallet",
            path.display()
        );
        assert!(
            !source.contains("chain::")
                && !source.contains("ElectrumBackend")
                && !source.contains("BitcoinCoreBackend"),
            "{} gives the cold signer a network client",
            path.display()
        );
    }
}

#[test]
fn cli_device_dispatch_uses_only_its_device_library_and_core() {
    let main = source("src/main.rs");
    let phone = main
        .split("fn run_phone(")
        .nth(1)
        .unwrap()
        .split("fn run_hww(")
        .next()
        .unwrap();
    assert!(!phone.contains("cold_wallet::"));
    assert!(!phone.contains("core::recovery::sweep"));

    let hww = main
        .split("fn run_hww(")
        .nth(1)
        .unwrap()
        .split("fn phone_set_policy(")
        .next()
        .unwrap();
    assert!(!hww.contains("hot_wallet::"));
    assert!(!hww.contains("core::recovery::sweep"));
}
