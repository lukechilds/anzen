//! Plain protocol data shared across chain adapters and offline signers.

use bitcoin::{OutPoint, TxOut};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultUtxo {
    pub outpoint: OutPoint,
    pub txout: TxOut,
    pub confirmation_height: u64,
}
