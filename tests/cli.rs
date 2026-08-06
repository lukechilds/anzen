use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn binary_and_help_use_the_anzen_name() {
    Command::cargo_bin("anzen")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: anzen"))
        .stdout(predicate::str::contains("--chain-backend <CHAIN_BACKEND>"))
        .stdout(predicate::str::contains("--rpc-url <RPC_URL>"))
        .stdout(predicate::str::contains("--electrum-url <ELECTRUM_URL>"));
}

#[test]
fn device_setup_and_vault_init_are_separate_and_monthly_spending_starts_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "phone", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Simulated phone initialized"))
        .stdout(predicate::str::contains("Phone mnemonic:"))
        .stdout(predicate::str::contains("Phone vault key:"))
        .stdout(predicate::str::contains("descriptor:").not());

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "hww", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Simulated HWW initialized"))
        .stdout(predicate::str::contains("HWW mnemonic:"))
        .stdout(predicate::str::contains("HWW vault key:"));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cold storage descriptor: tr("))
        .stdout(predicate::str::contains(
            "Phone recovery: 61,200 blocks (~14 months)",
        ))
        .stdout(predicate::str::contains(
            "HWW recovery:   65,535 blocks (~15 months)",
        ))
        .stdout(predicate::str::contains("Monthly spending: disabled"))
        .stdout(predicate::str::contains("Emergency access: disabled"))
        .stdout(predicate::str::contains("hot external descriptor").not())
        .stdout(predicate::str::contains("Hard limit").not());

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "policy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("older(61200)"))
        .stdout(predicate::str::contains("older(65535)"))
        .stdout(predicate::str::contains("Vault address: bcrt1p"))
        .stdout(predicate::str::contains("Monthly spending: disabled"))
        .stdout(predicate::str::contains("Emergency access: disabled"));
}

#[test]
fn phone_cli_exposes_the_complete_emergency_access_lifecycle() {
    Command::cargo_bin("anzen")
        .unwrap()
        .args(["phone", "set-policy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--emergency-access-limit"));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["phone", "emergency", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("initiate"))
        .stdout(predicate::str::contains("withdraw"))
        .stdout(predicate::str::contains("cancel"));
}

#[test]
fn hww_can_be_initialized_first_for_a_future_vanity_phone() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "hww", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Simulated HWW initialized"));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "phone", "init"])
        .assert()
        .success();

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "init"])
        .assert()
        .success();
}

#[test]
fn vanity_phone_init_requires_an_initialized_hww() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "phone", "init", "--vanity"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "initialize the HWW before using phone init --vanity",
        ));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["phone", "init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--vanity"));
}

#[test]
fn legacy_config_filename_remains_readable() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();

    for args in [["phone", "init"], ["hww", "init"], ["init", ""]] {
        let args = args.into_iter().filter(|arg| !arg.is_empty());
        Command::cargo_bin("anzen")
            .unwrap()
            .arg("--data-dir")
            .arg(data_dir)
            .args(args)
            .assert()
            .success();
    }

    std::fs::rename(dir.path().join("anzen.json"), dir.path().join("vault.json")).unwrap();

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "policy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Cold storage descriptor:"));
}

#[test]
fn legacy_bundled_device_commands_are_not_exposed() {
    for command in [
        "ceremony",
        "monthly",
        "soft-limit",
        "hot-address",
        "restore-phone",
        "sweep",
        "rotate-phone",
    ] {
        Command::cargo_bin("anzen")
            .unwrap()
            .arg(command)
            .assert()
            .failure()
            .stderr(predicate::str::contains("unrecognized subcommand"));
    }
}

#[test]
fn mainnet_mode_is_persisted_and_requires_the_dangerous_flag_every_time() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();
    let danger = "--dangerously-enable-mainnet";

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, danger, "phone", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MAINNET — REAL FUNDS"));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "hww", "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "pass --dangerously-enable-mainnet on every command",
        ));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, danger, "hww", "init"])
        .assert()
        .success();

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "pass --dangerously-enable-mainnet on every command",
        ));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, danger, "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Vault address: bc1p"))
        .stdout(predicate::str::contains("fixed 1 sat/vB MVP fees"));

    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("anzen.json")).unwrap()).unwrap();
    assert_eq!(config["network"], "mainnet");

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "policy"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("mainnet vault is locked"));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, danger, "policy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Vault address: bc1p"));
}

#[test]
fn dangerous_mainnet_flag_cannot_convert_an_existing_regtest_vault() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "phone", "init"])
        .assert()
        .success();

    Command::cargo_bin("anzen")
        .unwrap()
        .args([
            "--data-dir",
            data_dir,
            "--dangerously-enable-mainnet",
            "hww",
            "init",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot change an existing vault"));
}

#[test]
fn openpgp_friend_decrypts_the_descriptor_bound_cloud_backup() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();
    for args in [vec!["phone", "init"], vec!["hww", "init"], vec!["init"]] {
        Command::cargo_bin("anzen")
            .unwrap()
            .args(["--data-dir", data_dir])
            .args(args)
            .assert()
            .success();
    }

    let public = dir.path().join("alice.pub.asc");
    let private = dir.path().join("alice.sec.asc");
    let recovery = dir.path().join("friend-recovery.json");
    let backup = dir.path().join("cloud/phone-seed-backup.json");
    Command::cargo_bin("anzen")
        .unwrap()
        .args([
            "--data-dir",
            data_dir,
            "social",
            "generate-friend-key",
            "--name",
            "Alice",
        ])
        .arg("--public-key")
        .arg(&public)
        .arg("--private-key")
        .arg(&private)
        .assert()
        .success()
        .stdout(predicate::str::contains("OpenPGP key generated"));
    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "hww", "add-recovery-friend"])
        .arg(&public)
        .arg("--yes")
        .assert()
        .success()
        .stdout(predicate::str::contains("Recovery friend added"));

    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("anzen.json")).unwrap()).unwrap();
    let backup_text = std::fs::read_to_string(&backup).unwrap();
    let backup_json: serde_json::Value = serde_json::from_str(&backup_text).unwrap();
    assert_eq!(backup_json["friends"].as_array().unwrap().len(), 1);
    assert!(!backup_text.contains(config["vault_descriptor"].as_str().unwrap()));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "social", "decrypt-backup"])
        .arg(&backup)
        .arg("--private-key")
        .arg(&private)
        .arg("--output")
        .arg(&recovery)
        .assert()
        .success()
        .stdout(predicate::str::contains("Social recovery decrypted"))
        .stdout(predicate::str::contains("Cold storage descriptor: tr("));
    let recovered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&recovery).unwrap()).unwrap();
    assert_eq!(recovered["vault_descriptor"], config["vault_descriptor"]);

    std::fs::remove_file(dir.path().join("phone/device.json")).unwrap();
    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "phone", "restore"])
        .arg(&recovery)
        .assert()
        .success()
        .stdout(predicate::str::contains("Phone key restored"));
}
