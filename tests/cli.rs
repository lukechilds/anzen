use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn device_setup_and_vault_init_are_separate_and_monthly_spending_starts_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().to_str().unwrap();

    Command::cargo_bin("vault")
        .unwrap()
        .args(["--data-dir", data_dir, "phone", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Simulated phone initialized"))
        .stdout(predicate::str::contains("Phone mnemonic:"))
        .stdout(predicate::str::contains("Phone vault key:"))
        .stdout(predicate::str::contains("descriptor:").not());

    Command::cargo_bin("vault")
        .unwrap()
        .args(["--data-dir", data_dir, "hww", "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Simulated HWW initialized"))
        .stdout(predicate::str::contains("HWW mnemonic:"))
        .stdout(predicate::str::contains("HWW vault key:"));

    Command::cargo_bin("vault")
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
        .stdout(predicate::str::contains("hot external descriptor").not())
        .stdout(predicate::str::contains("Hard limit").not());

    Command::cargo_bin("vault")
        .unwrap()
        .args(["--data-dir", data_dir, "policy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("older(61200)"))
        .stdout(predicate::str::contains("older(65535)"))
        .stdout(predicate::str::contains("Vault address: bcrt1p"))
        .stdout(predicate::str::contains("Monthly spending: disabled"));
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
        Command::cargo_bin("vault")
            .unwrap()
            .arg(command)
            .assert()
            .failure()
            .stderr(predicate::str::contains("unrecognized subcommand"));
    }
}
