//! The app's protocol messages, answered by a hardware signer.
//!
//! The phone app talks to its signer in versioned JSON messages (`signer-hello`, `vault-setup`,
//! `policy-proposal`, …), the same over a tap, a file or Bluetooth. This turns each one into a
//! hardware signer app's commands and its answer back into the message a software signer would
//! have sent, so the app's flows do not know which kind of signer they are talking to.
//!
//! Proof of concept, like the hardware apps themselves: it sets up vaults and approves policies,
//! transfers and the signer's own actions. The apps cannot yet protect a recovery backup, and
//! this does not yet rotate a phone key or share a vault for recovery; those messages are
//! refused by name.

use anyhow::{Context, Result, bail, ensure};
use anzen_common::{
    protocol,
    storage::{PublicDeviceFile, VaultConfig},
};
use anzen_protocol::wire::{CooperativeSignature, Signature};
use anzen_protocol::{
    address::Network,
    script::XOnlyKey,
    wire::{
        self, OfflineActionApproval, OfflineActionRequest, PolicyApproval, PolicyRequest,
        SweepApproval, SweepRequest,
    },
};
use anzen_signer::{State, hardware::Pair};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

/// The person rejected the review on the device.
#[derive(Debug)]
pub struct Rejected;

impl std::fmt::Display for Rejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("rejected on the signer")
    }
}

impl std::error::Error for Rejected {}

/// A hardware signer app, as the bridge drives it. Each approval waits while the person reviews
/// the request on the device, and fails with [`Rejected`] if they turn it down.
pub trait Device: Send {
    /// What the device is called in the messages a person reads.
    const NAME: &'static str;
    /// Whether the device itself remembers its vault and refuses a second one. A device that
    /// stores nothing (its host carries its state) accepts whichever vault it is set up with.
    const KEEPS_ONE_VAULT: bool;

    /// The signer's vault key on `network`.
    fn identity(&mut self, network: Network) -> impl Future<Output = Result<XOnlyKey>> + Send;
    /// The vault the signer accepted on `network`, and the last policy it approved.
    fn state(&mut self, network: Network) -> impl Future<Output = Result<Option<State>>> + Send;
    /// Accept the vault of this phone key. Approves no spending.
    fn pair(&mut self, pair: &Pair) -> impl Future<Output = Result<()>> + Send;
    fn approve_policy(
        &mut self,
        request: &PolicyRequest,
    ) -> impl Future<Output = Result<Vec<CooperativeSignature>>> + Send;
    fn approve_sweep(
        &mut self,
        request: &SweepRequest,
    ) -> impl Future<Output = Result<Vec<CooperativeSignature>>> + Send;
    fn approve_offline_action(
        &mut self,
        request: &OfflineActionRequest,
    ) -> impl Future<Output = Result<Vec<Signature>>> + Send;
}

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

/// Answer one message from the phone app, as its signer. Reviews happen on the device's screen
/// while this waits.
pub async fn answer<D: Device>(device: &mut D, message: &Message) -> Result<Message> {
    let name = D::NAME;
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
            if D::KEEPS_ONE_VAULT
                && let (Some(state), Some(phone)) = (device.state(network).await?, &hello.phone)
            {
                ensure!(
                    state.keys.phone == XOnlyKey::from_hex(phone).context("invalid phone key")?,
                    "This {name} already protects another vault. Forget it in the Anzen app on the {name} first."
                );
            }
            let key = device.identity(network).await?;
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
        // The vault the phone built from both keys. The device rebuilds it from the phone key
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
                keys.hww == device.identity(network).await?,
                "this vault was built for another signer"
            );
            device
                .pair(&Pair {
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
            let signatures = device.approve_policy(&request).await?;
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
            let signatures = device.approve_sweep(&request).await?;
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
            let signatures = device.approve_offline_action(&request).await?;
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
            bail!("The {name} app cannot protect or unlock a recovery backup yet.")
        }
        "phone-rotation-proposal" | "phone-rotation-completed" => {
            bail!("The {name} app cannot move a vault to a new phone key yet.")
        }
        "recovery-vault-request" => {
            bail!("The {name} app cannot share a vault for recovery yet.")
        }
        other => bail!("a signer does not answer {other} messages"),
    }
}
