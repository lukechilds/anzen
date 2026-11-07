//! Wallet activity is derived from BDK's canonical transaction graph, never invented by the UI.
//! This intentionally does not open the vault configuration or reveal its private balance.

use bdk_wallet::{Wallet, chain::ChainPosition};
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Received,
    Sent,
    SelfTransfer,
}

#[derive(Debug, Serialize)]
pub struct WalletActivity {
    pub txid: String,
    pub kind: ActivityKind,
    /// The actual effect on this wallet's balance, including fees for outgoing payments.
    pub balance_change_sats: i64,
    pub confirmed: bool,
    pub timestamp: Option<u64>,
    pub fee_sats: Option<u64>,
}

pub fn wallet_activity(wallet: &Wallet) -> Vec<WalletActivity> {
    let mut activity = wallet
        .transactions()
        .map(|entry| {
            let tx = &entry.tx_node.tx;
            let (sent, received) = wallet.sent_and_received(tx);
            let kind = if sent.to_sat() == 0 {
                ActivityKind::Received
            } else if tx
                .output
                .iter()
                .all(|output| wallet.is_mine(output.script_pubkey.clone()))
            {
                ActivityKind::SelfTransfer
            } else {
                ActivityKind::Sent
            };
            let timestamp = match entry.chain_position {
                ChainPosition::Confirmed {
                    anchor,
                    transitively: None,
                } => Some(anchor.confirmation_time),
                // A child's anchor only establishes an upper bound, not this transaction's date.
                ChainPosition::Confirmed {
                    transitively: Some(_),
                    ..
                } => None,
                ChainPosition::Unconfirmed {
                    first_seen,
                    last_seen,
                } => first_seen.or(last_seen),
            };
            WalletActivity {
                txid: tx.compute_txid().to_string(),
                kind,
                balance_change_sats: received.to_sat() as i64 - sent.to_sat() as i64,
                confirmed: entry.chain_position.is_confirmed(),
                timestamp,
                fee_sats: wallet.calculate_fee(tx).ok().map(|fee| fee.to_sat()),
            }
        })
        .collect::<Vec<_>>();
    activity.sort_by(|a, b| {
        a.confirmed
            .cmp(&b.confirmed)
            .then_with(|| b.timestamp.cmp(&a.timestamp))
            .then_with(|| a.txid.cmp(&b.txid))
    });
    activity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HotWallet;
    use anzen_common::keys::DeviceKeys;
    use bitcoin::{
        Amount, Network, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid, hashes::Hash,
    };

    #[test]
    fn canonical_history_reports_real_net_amounts_and_excludes_unrelated_outputs() {
        let keys = DeviceKeys::generate_for_network(
            &bitcoin::secp256k1::Secp256k1::new(),
            Network::Regtest,
        )
        .unwrap();
        let mut hot = HotWallet::ephemeral(&keys).unwrap();
        let address = hot.next_receive_address().unwrap();
        let incoming = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([12; 32]), 0),
                ..Default::default()
            }],
            output: vec![TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: address.script_pubkey(),
            }],
        };
        hot.wallet.apply_unconfirmed_txs([(incoming.clone(), 100)]);
        let received = wallet_activity(&hot.wallet);
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].balance_change_sats, 100_000);
        assert_eq!(received[0].kind, ActivityKind::Received);
        assert!(!received[0].confirmed);
        let outgoing = Transaction {
            input: vec![TxIn {
                previous_output: OutPoint::new(incoming.compute_txid(), 0),
                ..Default::default()
            }],
            output: vec![
                TxOut {
                    value: Amount::from_sat(60_000),
                    script_pubkey: ScriptBuf::new(),
                },
                TxOut {
                    value: Amount::from_sat(39_000),
                    script_pubkey: address.script_pubkey(),
                },
            ],
            ..incoming
        };
        hot.wallet.apply_unconfirmed_txs([(outgoing, 200)]);
        let history = wallet_activity(&hot.wallet);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].kind, ActivityKind::Sent);
        assert_eq!(history[0].balance_change_sats, -61_000);
        assert_eq!(history[0].fee_sats, Some(1_000));
        assert_eq!(history[0].timestamp, Some(200));
    }
}
