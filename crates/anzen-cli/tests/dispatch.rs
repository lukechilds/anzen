//! The CLI is the one binary that links both devices, so the compiler cannot keep a phone command
//! from calling signer code or the reverse. Crate-level boundaries are enforced from the resolved
//! dependency graph by `scripts/check-boundaries.sh`; this covers only the command dispatch.

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split(start)
        .nth(1)
        .and_then(|rest| rest.split(end).next())
        .unwrap_or_else(|| panic!("{start} not found before {end}"))
}

#[test]
fn each_device_command_uses_only_its_own_device_library() {
    let source = include_str!("../src/main.rs");
    let phone = section(source, "fn run_phone(", "fn run_hww(");
    assert!(!phone.contains("cold_wallet::") && !phone.contains("SignerApplication"));
    let hww = section(source, "fn run_hww(", "fn run_social(");
    assert!(!hww.contains("hot_wallet::") && !hww.contains("WalletApplication"));
}
