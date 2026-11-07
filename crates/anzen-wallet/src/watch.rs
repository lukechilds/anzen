//! Live updates: which public scripts to watch, and the listener that turns chain activity into
//! "something changed" hints, shared by every platform.
//!
//! The listener does no I/O. A platform reports what happens (a connection opened or closed, a
//! frame arrived, time passed, regtest activity) and carries out the returned [`Action`]s:
//! JavaScript in the browser's I/O worker, a Rust thread natively. Hints only trigger the
//! ordinary sync; a missing or spurious hint can delay or repeat a refresh but never change
//! wallet state.

use super::HotWallet;
use anyhow::Result;
use bdk_wallet::KeychainKind;
use bitcoin::{
    Script,
    hashes::{Hash, sha256},
};
use serde::Serialize;

#[cfg(feature = "electrum")]
mod listener;
#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "electrum")]
pub use listener::{
    Action, ActivityTracker, ChainWatcher, Change, ElectrumWatcher, RegtestActivity,
};

/// Unrevealed addresses to watch past each keychain's last revealed index. A payment to an
/// address this wallet has not revealed is still found by the next ordinary sync.
pub const LOOKAHEAD: u32 = 5;

#[derive(Debug, Serialize)]
pub struct WatchTargets {
    /// Electrum script hashes: SHA-256 of the output script, hex encoded in reversed byte order.
    pub scripthashes: Vec<String>,
}

pub fn electrum_scripthash(script: &Script) -> String {
    let mut hash = sha256::Hash::hash(script.as_bytes()).to_byte_array();
    hash.reverse();
    bitcoin::hex::DisplayHex::to_lower_hex_string(&hash[..])
}

/// The hot wallet's revealed addresses plus a short lookahead, and the vault and controller
/// addresses once paired. The vault is watched so an open vault view can refresh; whether the
/// vault is shown remains a presentation decision.
pub fn targets(wallet: &HotWallet, vault_scripts: &[&Script]) -> Result<WatchTargets> {
    let mut scripthashes = Vec::new();
    for keychain in [KeychainKind::External, KeychainKind::Internal] {
        let last = wallet.wallet.derivation_index(keychain);
        let end = last.map_or(LOOKAHEAD, |index| index.saturating_add(LOOKAHEAD + 1));
        for index in 0..end {
            let address = wallet.wallet.peek_address(keychain, index).address;
            scripthashes.push(electrum_scripthash(&address.script_pubkey()));
        }
    }
    scripthashes.extend(
        vault_scripts
            .iter()
            .map(|script| electrum_scripthash(script)),
    );
    scripthashes.sort_unstable();
    scripthashes.dedup();
    Ok(WatchTargets { scripthashes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::WalletApplication;
    use bdk_electrum::electrum_client::ToElectrumScriptHash;
    use bitcoin::{Address, Network};
    use std::str::FromStr;

    #[test]
    fn targets_follow_revealed_addresses_and_include_the_paired_vault() {
        let dir = anzen_common::storage::tempdir().unwrap();
        let config = crate::test_support::initialize(dir.path()).unwrap().config;
        let app = WalletApplication::new(dir.path().into(), false);
        let vault = Address::from_str(&config.vault_address)
            .unwrap()
            .require_network(Network::Regtest)
            .unwrap()
            .script_pubkey();
        // Byte order matches what the Electrum client library sends on the wire.
        assert_eq!(
            serde_json::json!(electrum_scripthash(&vault)),
            serde_json::to_value(vault.to_electrum_scripthash()).unwrap()
        );

        let before = app.watch_targets().unwrap().scripthashes;
        // Lookahead on both keychains, plus the vault and controller addresses.
        assert_eq!(before.len(), 2 * LOOKAHEAD as usize + 2);
        assert!(before.contains(&electrum_scripthash(&vault)));

        let mut addresses = Vec::new();
        for _ in 0..=LOOKAHEAD {
            addresses.push(app.receive_address().unwrap());
        }
        let after = app.watch_targets().unwrap().scripthashes;
        assert_eq!(after.len(), before.len() + LOOKAHEAD as usize + 1);
        for address in addresses {
            let script = Address::from_str(&address)
                .unwrap()
                .assume_checked()
                .script_pubkey();
            assert!(after.contains(&electrum_scripthash(&script)));
        }
    }
}
