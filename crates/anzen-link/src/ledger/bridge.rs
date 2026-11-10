//! The app's protocol messages, answered by a Ledger.
//!
//! The phone app talks to its signer in versioned JSON messages (`signer-hello`, `vault-setup`,
//! `policy-proposal`, …), the same over a tap, a file or Bluetooth. This turns each one into the
//! Ledger app's commands and its answer back into the message a software signer would have sent,
//! so the app's flows do not know which kind of signer they are talking to.
//!
//! Proof of concept, like the Ledger app itself: it sets up vaults and approves policies,
//! transfers and the signer's own actions. The Ledger app cannot yet protect a recovery backup,
//! rotate a phone key or share a vault for recovery, and those messages are refused by name.

use super::{Exchange, HardwareSigner};
use anyhow::{Context, Result, bail, ensure};
use anzen_common::{
    protocol,
    storage::{PublicDeviceFile, VaultConfig},
};
use anzen_protocol::{
    address::Network,
    script::XOnlyKey,
    wire::{
        self, OfflineActionApproval, OfflineActionRequest, PolicyApproval, PolicyRequest,
        SweepApproval, SweepRequest,
    },
};
use anzen_signer::hardware::Pair;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

/// One protocol message: its kind and its JSON body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub kind: String,
    pub body: Value,
}

impl Message {
    fn new(kind: &str, body: impl Serialize) -> Result<Self> {
        Ok(Self {
            kind: kind.to_owned(),
            body: serde_json::to_value(body)?,
        })
    }

    fn parse<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.body.clone())
            .with_context(|| format!("unreadable {} message", self.kind))
    }
}

/// The app names networks as Bitcoin does; stored files use Anzen's own names.
fn network(name: &str) -> Result<Network> {
    Ok(match name {
        "bitcoin" | "mainnet" => Network::Mainnet,
        "signet" | "mutinynet" => Network::Mutinynet,
        "regtest" => Network::Regtest,
        other => bail!("unsupported network {other}"),
    })
}

impl<T: Exchange> HardwareSigner<T> {
    /// Answer one message from the phone app, as its signer. Reviews happen on the Ledger's
    /// screen while this waits.
    pub async fn answer(&mut self, message: &Message) -> Result<Message> {
        match message.kind.as_str() {
            // The phone introduces itself; the signer answers with its public key.
            "signer-hello" => {
                #[derive(Deserialize)]
                struct Hello {
                    network: String,
                    phone: Option<String>,
                }
                let hello: Hello = message.parse()?;
                let network = network(&hello.network)?;
                if let (Some(state), Some(phone)) = (self.state(network).await?, &hello.phone) {
                    ensure!(
                        state.keys.phone
                            == XOnlyKey::from_hex(phone).context("invalid phone key")?,
                        "This Ledger already protects another vault. Forget it in the Anzen app on the Ledger first."
                    );
                }
                let key = self.identity(network).await?;
                Message::new(
                    "signer-identity",
                    PublicDeviceFile {
                        version: 1,
                        kind: "hww-public-key".to_owned(),
                        // Anzen's own network names, as stored files use them.
                        network: network.name().to_owned(),
                        vault_pubkey: key.to_hex(),
                    },
                )
            }
            // The vault the phone built from both keys. The Ledger rebuilds it from the phone key
            // and its own; it cannot yet wrap the phone's recovery backup, so the answer has none.
            "vault-setup" => {
                #[derive(Deserialize)]
                struct Setup {
                    config: VaultConfig,
                }
                let Setup { config } = message.parse()?;
                config.validate_public_policy()?;
                let network = protocol::config_network(&config)?;
                let keys = protocol::vault_keys(&config)?;
                ensure!(
                    keys.hww == self.identity(network).await?,
                    "this vault was built for another signer"
                );
                self.pair(&Pair {
                    network,
                    phone: keys.phone,
                    recovery_locktime: keys.recovery_locktime,
                })
                .await?;
                Message::new(
                    "vault-accepted",
                    json!({ "vault_address": config.vault_address }),
                )
            }
            "policy-proposal" => {
                let request: PolicyRequest = message.parse()?;
                let signatures = self.approve_policy(&request).await?;
                Message::new(
                    "policy-approved",
                    PolicyApproval {
                        version: wire::VERSION,
                        request,
                        signatures,
                    },
                )
            }
            "sweep-proposal" => {
                let request: SweepRequest = message.parse()?;
                let signatures = self.approve_sweep(&request).await?;
                Message::new(
                    "sweep-approved",
                    SweepApproval {
                        version: wire::VERSION,
                        request,
                        signatures,
                    },
                )
            }
            "offline-action-proposal" => {
                let request: OfflineActionRequest = message.parse()?;
                let signatures = self.approve_offline_action(&request).await?;
                Message::new(
                    "offline-action-approved",
                    OfflineActionApproval {
                        version: wire::VERSION,
                        request,
                        signatures,
                    },
                )
            }
            "backup-proposal" | "restore-proposal" => {
                bail!("The Ledger app cannot protect or unlock a recovery backup yet.")
            }
            "phone-rotation-proposal" | "phone-rotation-completed" => {
                bail!("The Ledger app cannot move a vault to a new phone key yet.")
            }
            "recovery-vault-request" => {
                bail!("The Ledger app cannot share a vault for recovery yet.")
            }
            other => bail!("a signer does not answer {other} messages"),
        }
    }
}
