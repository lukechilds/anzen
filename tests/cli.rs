use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn init_and_policy_commands_show_the_regtest_policy() {
    let dir = tempfile::tempdir().unwrap();
    Command::cargo_bin("vault-cli")
        .unwrap()
        .args(["--data-dir", dir.path().to_str().unwrap(), "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Vault initialized (REGTEST ONLY)"))
        .stdout(predicate::str::contains("Phone mnemonic:"))
        .stdout(predicate::str::contains("HWW mnemonic:"))
        .stdout(predicate::str::contains(
            "Phone hot external descriptor: tr(",
        ))
        .stdout(predicate::str::contains(
            "Phone hot change descriptor:   tr(",
        ))
        .stdout(predicate::str::contains("Phone recovery: 61200 blocks"))
        .stdout(predicate::str::contains("HWW recovery:   65535 blocks"))
        .stdout(predicate::str::contains("Hard limit:     10000000 sats"));

    Command::cargo_bin("vault-cli")
        .unwrap()
        .args(["--data-dir", dir.path().to_str().unwrap(), "policy"])
        .assert()
        .success()
        .stdout(predicate::str::contains("older(61200)"))
        .stdout(predicate::str::contains("older(65535)"))
        .stdout(predicate::str::contains("Vault address: bcrt1p"));
}
