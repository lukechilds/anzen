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
            "HWW alone:      after 33,553,920 seconds (~388 days)",
        ))
        .stdout(predicate::str::contains("Phone recovery").not())
        .stdout(predicate::str::contains("Monthly spending: disabled"))
        .stdout(predicate::str::contains("Emergency access: disabled"))
        .stdout(predicate::str::contains("hot external descriptor").not())
        .stdout(predicate::str::contains("Hard limit").not());

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["--data-dir", data_dir, "policy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("and_v(v:older(4259839),pk("))
        .stdout(predicate::str::contains("older(61200)").not())
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

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["phone", "rotate-key", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--vanity"))
        .stdout(predicate::str::contains("--output"));
}

#[test]
fn phone_cli_addresses_allowances_by_sequential_step() {
    for command in ["authorize", "revoke", "apply-soft-limit"] {
        Command::cargo_bin("anzen")
            .unwrap()
            .args(["phone", command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("<STEP>"))
            .stdout(predicate::str::contains("<MONTH>").not());
    }
}

#[test]
fn hww_cli_exposes_blanket_controller_revocation() {
    Command::cargo_bin("anzen")
        .unwrap()
        .args(["hww", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("revoke-policy"));

    Command::cargo_bin("anzen")
        .unwrap()
        .args(["hww", "revoke-policy", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--yes"))
        .stdout(predicate::str::contains("destination"));
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
        .args([
            "--data-dir",
            data_dir,
            "phone",
            "rotate-key",
            "--vanity",
            "--output",
            "rotation.json",
        ])
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
fn the_cloud_backup_hides_the_descriptor_and_only_the_hww_can_open_it() {
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
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("anzen.json")).unwrap()).unwrap();
    let backup_text =
        std::fs::read_to_string(dir.path().join("cloud/phone-seed-backup.json")).unwrap();
    let backup_json: serde_json::Value = serde_json::from_str(&backup_text).unwrap();
    assert_eq!(backup_json["version"], 2);
    assert!(backup_json.get("friends").is_none());
    assert!(!backup_text.contains(config["vault_descriptor"].as_str().unwrap()));
    assert!(!backup_text.contains(config["phone_vault_pubkey"].as_str().unwrap()));
}

#[test]
fn mutinynet_is_chosen_once_and_every_later_command_follows_it() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();
    let anzen = || {
        let mut command = Command::cargo_bin("anzen").unwrap();
        command
            .env_remove("ANZEN_NETWORK")
            .args(["--data-dir", data_dir]);
        command
    };
    anzen()
        .args(["--network", "mutinynet", "hww", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MUTINYNET — TEST COINS"));
    // The phone joins the signer's network without repeating the option.
    anzen()
        .args(["phone", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("MUTINYNET — TEST COINS"));
    anzen()
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("Vault address: tb1p"));
    anzen()
        .args(["--network", "regtest", "policy"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "vault is configured for mutinynet; --network cannot change an existing vault",
        ));
    anzen()
        .args(["--dangerously-enable-mainnet", "policy"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot change an existing vault"));
    anzen()
        .args([
            "--network",
            "mutinynet",
            "--dangerously-enable-mainnet",
            "policy",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be combined with mainnet"));
}
