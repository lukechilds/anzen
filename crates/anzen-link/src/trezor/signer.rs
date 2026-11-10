//! The Anzen app on a Trezor, as a signer: each call sends one request, waits while the person
//! reviews it on the device, and returns the signatures. Nothing here can approve anything.
//!
//! The app stores nothing, so this end keeps what the signer would have stored (the vault it
//! accepted, the last policy it approved, a rotation it approved) under the signer's seal, and
//! hands it back with each request ([`anzen_signer::hardware::stateless`]).

use super::{AppImage, Transport, Trezor, thp::Failure};
use crate::bridge::{self, Device, Message, Rejected};
use anyhow::{Context, Result, bail, ensure};
use anzen_protocol::{
    address::Network,
    script::XOnlyKey,
    wire::{
        CooperativeSignature, OfflineActionRequest, PolicyRequest, RotationRequest, Signature,
        SweepRequest,
    },
};
use anzen_signer::{
    State,
    hardware::{
        Operation, Pair, RotationSignatures, network_code,
        stateless::{
            self, Call, MAX_REQUEST, Outcome, PIECE, PROTOCOL_VERSION, Sealed, answer, message,
        },
    },
};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::HashMap, path::PathBuf};

/// Which of the signer's two memories a sealed state is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    /// The vault the signer accepted, and the last policy it approved.
    Vault,
    /// The vault of an approved phone rotation, not yet adopted.
    Rotated,
}

/// Where the host keeps what the signer sealed for it. Losing it is not fatal: the vault is set
/// up again from the same two keys, and the signer starts over with no approved policy.
pub trait Memory: Send {
    fn load(&self, network: Network, slot: Slot) -> Result<Option<Sealed>>;
    /// `None` forgets.
    fn store(&mut self, network: Network, slot: Slot, sealed: Option<&Sealed>) -> Result<()>;
}

/// Memory that lasts as long as the process: for tests.
#[derive(Default)]
pub struct Volatile(HashMap<(u8, Slot), Sealed>);

impl Memory for Volatile {
    fn load(&self, network: Network, slot: Slot) -> Result<Option<Sealed>> {
        Ok(self.0.get(&(network_code(network), slot)).cloned())
    }

    fn store(&mut self, network: Network, slot: Slot, sealed: Option<&Sealed>) -> Result<()> {
        let key = (network_code(network), slot);
        match sealed {
            Some(sealed) => self.0.insert(key, sealed.clone()),
            None => self.0.remove(&key),
        };
        Ok(())
    }
}

/// Memory in a directory: one small file for each network and slot.
pub struct Directory(pub PathBuf);

impl Directory {
    fn path(&self, network: Network, slot: Slot) -> PathBuf {
        let slot = match slot {
            Slot::Vault => "vault",
            Slot::Rotated => "rotated",
        };
        self.0
            .join(format!("trezor-{}-{slot}.sealed", network.name()))
    }
}

impl Memory for Directory {
    fn load(&self, network: Network, slot: Slot) -> Result<Option<Sealed>> {
        match std::fs::read(self.path(network, slot)) {
            // An unreadable file is no memory at all, which the signer treats as a fresh start.
            Ok(bytes) => Ok(postcard::from_bytes(&bytes).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("could not read the Trezor's stored vault"),
        }
    }

    fn store(&mut self, network: Network, slot: Slot, sealed: Option<&Sealed>) -> Result<()> {
        let path = self.path(network, slot);
        match sealed {
            Some(sealed) => {
                std::fs::create_dir_all(&self.0)?;
                // Written beside the file and renamed over it, so a crash leaves the old state
                // or the new one, never half of either.
                let scratch = path.with_extension("sealed.new");
                std::fs::write(&scratch, postcard::to_allocvec(sealed)?)?;
                std::fs::rename(&scratch, &path).context("could not store the Trezor's vault")?;
            }
            None => match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error).context("could not forget the Trezor's vault");
                }
                _ => {}
            },
        }
        Ok(())
    }
}

pub struct TrezorSigner<T, M> {
    trezor: Trezor<T>,
    instance: u32,
    memory: M,
}

impl<T: Transport, M: Memory> TrezorSigner<T, M> {
    /// Load the Anzen app onto a Trezor whose channel is paired and whose wallet is open, and
    /// check it speaks this protocol.
    pub async fn open(mut trezor: Trezor<T>, image: &AppImage, memory: M) -> Result<Self> {
        let instance = trezor.load_app(image).await?;
        let mut signer = Self {
            trezor,
            instance,
            memory,
        };
        signer.check().await?;
        Ok(signer)
    }

    /// Whether the loaded app still answers, and in this protocol.
    pub async fn check(&mut self) -> Result<()> {
        let version = self.send(message::INFO, &[]).await?;
        ensure!(
            version == [PROTOCOL_VERSION],
            "The Anzen app for your Trezor is a different version: update this app."
        );
        Ok(())
    }

    pub fn into_trezor(self) -> Trezor<T> {
        self.trezor
    }

    pub fn trezor(&mut self) -> &mut Trezor<T> {
        &mut self.trezor
    }

    /// The signer's vault key on `network`.
    pub async fn identity(&mut self, network: Network) -> Result<XOnlyKey> {
        let key = self
            .send(message::IDENTITY, &[network_code(network)])
            .await?;
        Ok(XOnlyKey(
            key.as_slice()
                .try_into()
                .context("the Trezor's key is not 32 bytes")?,
        ))
    }

    /// The vault the signer accepted on `network` and the last policy it approved, as this host
    /// carries it. The Trezor is not asked: it holds nothing to ask for.
    pub fn state(&self, network: Network) -> Result<Option<State>> {
        self.memory
            .load(network, Slot::Vault)?
            .map(|sealed| decode(&sealed.state))
            .transpose()
    }

    /// Accept the vault of this phone key. The Trezor asks the person to confirm the vault's
    /// address, unless this host already carries that very vault. Approves no spending.
    pub async fn pair(&mut self, pair: &Pair) -> Result<()> {
        self.run(
            Operation::Pair,
            pair.network,
            Slot::Vault,
            Slot::Vault,
            encode(pair)?,
        )
        .await
        .map(drop)
    }

    pub async fn approve_policy(
        &mut self,
        request: &PolicyRequest,
    ) -> Result<Vec<CooperativeSignature>> {
        decode(
            &self
                .run(
                    Operation::Policy,
                    request.network,
                    Slot::Vault,
                    Slot::Vault,
                    encode(request)?,
                )
                .await?,
        )
    }

    pub async fn approve_sweep(
        &mut self,
        request: &SweepRequest,
    ) -> Result<Vec<CooperativeSignature>> {
        decode(
            &self
                .run(
                    Operation::Sweep,
                    request.network,
                    Slot::Vault,
                    Slot::Vault,
                    encode(request)?,
                )
                .await?,
        )
    }

    pub async fn approve_offline_action(
        &mut self,
        request: &OfflineActionRequest,
    ) -> Result<Vec<Signature>> {
        decode(
            &self
                .run(
                    Operation::OfflineAction,
                    request.network,
                    Slot::Vault,
                    Slot::Vault,
                    encode(request)?,
                )
                .await?,
        )
    }

    /// Approve moving the vault to a new phone key. What comes back sealed is the new vault's
    /// state, kept aside until [`Self::adopt_rotation`].
    pub async fn approve_rotation(
        &mut self,
        request: &RotationRequest,
    ) -> Result<RotationSignatures> {
        decode(
            &self
                .run(
                    Operation::Rotation,
                    request.network,
                    Slot::Vault,
                    Slot::Rotated,
                    encode(request)?,
                )
                .await?,
        )
    }

    /// Switch to the vault of the last approved rotation, once it has confirmed.
    pub async fn adopt_rotation(&mut self, network: Network) -> Result<()> {
        self.run(
            Operation::AdoptRotation,
            network,
            Slot::Rotated,
            Slot::Vault,
            vec![network_code(network)],
        )
        .await?;
        self.memory.store(network, Slot::Rotated, None)
    }

    pub async fn forget_policy(&mut self, network: Network) -> Result<()> {
        self.run(
            Operation::ForgetPolicy,
            network,
            Slot::Vault,
            Slot::Vault,
            vec![network_code(network)],
        )
        .await
        .map(drop)
    }

    /// Forget the vault on `network`. The Trezor holds nothing, so this only drops what the host
    /// carried; the same phone can set the vault up again.
    pub fn forget_vault(&mut self, network: Network) -> Result<()> {
        self.memory.store(network, Slot::Vault, None)?;
        self.memory.store(network, Slot::Rotated, None)
    }

    /// Run one operation: hand the signer what it sealed in `carried`, and keep what it seals in
    /// answer in `kept`.
    async fn run(
        &mut self,
        operation: Operation,
        network: Network,
        carried: Slot,
        kept: Slot,
        request: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let call = Call {
            carried: self.memory.load(network, carried)?,
            request,
        };
        let outcome: Outcome = decode(
            &self
                .send(stateless::run(operation), &encode(&call)?)
                .await?,
        )?;
        if let Some(sealed) = &outcome.carry {
            self.memory.store(network, kept, Some(sealed))?;
        }
        Ok(outcome.answer)
    }

    /// One request and its whole answer. A body too long for one message travels in pieces,
    /// each acknowledged before the next.
    async fn send(&mut self, id: u16, data: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            data.len() <= MAX_REQUEST,
            "the request is too large for a Trezor"
        );
        let mut rest = data;
        while rest.len() > PIECE {
            let (piece, after) = rest.split_at(PIECE);
            let (status, refusal) = self.message(message::PART, piece).await?;
            match status {
                answer::MORE => rest = after,
                answer::REFUSED => bail!("{}", String::from_utf8_lossy(&refusal)),
                other => bail!("the Anzen app on the Trezor answered with message {other}"),
            }
        }
        let (mut status, mut answered) = self.message(id, rest).await?;
        while status == answer::PART {
            let (next, piece) = self.message(message::NEXT, &[]).await?;
            answered.extend(piece);
            status = next;
        }
        match status {
            answer::OK => Ok(answered),
            answer::REJECTED => Err(Rejected.into()),
            answer::REFUSED => bail!("{}", String::from_utf8_lossy(&answered)),
            other => bail!("the Anzen app on the Trezor answered with message {other}"),
        }
    }

    async fn message(&mut self, id: u16, data: &[u8]) -> Result<(u16, Vec<u8>)> {
        match self.trezor.app_call(self.instance, id, data).await {
            // Cancelling a screen of the Trezor's own (its PIN, say) is a rejection too.
            Err(error)
                if error
                    .downcast_ref::<Failure>()
                    .is_some_and(Failure::is_cancelled) =>
            {
                Err(Rejected.into())
            }
            answered => answered,
        }
    }

    /// Answer one message from the phone app, as its signer ([`bridge::answer`]).
    pub async fn answer(&mut self, message: &Message) -> Result<Message> {
        bridge::answer(self, message).await
    }
}

impl<T: Transport, M: Memory> Device for TrezorSigner<T, M> {
    const NAME: &'static str = "Trezor";
    const KEEPS_ONE_VAULT: bool = false;

    async fn identity(&mut self, network: Network) -> Result<XOnlyKey> {
        Self::identity(self, network).await
    }

    async fn state(&mut self, network: Network) -> Result<Option<State>> {
        Self::state(self, network)
    }

    async fn pair(&mut self, pair: &Pair) -> Result<()> {
        Self::pair(self, pair).await
    }

    async fn approve_policy(
        &mut self,
        request: &PolicyRequest,
    ) -> Result<Vec<CooperativeSignature>> {
        Self::approve_policy(self, request).await
    }

    async fn approve_sweep(&mut self, request: &SweepRequest) -> Result<Vec<CooperativeSignature>> {
        Self::approve_sweep(self, request).await
    }

    async fn approve_offline_action(
        &mut self,
        request: &OfflineActionRequest,
    ) -> Result<Vec<Signature>> {
        Self::approve_offline_action(self, request).await
    }
}

fn encode<M: Serialize>(message: &M) -> Result<Vec<u8>> {
    postcard::to_allocvec(message).context("could not encode the request")
}

fn decode<M: DeserializeOwned>(answer: &[u8]) -> Result<M> {
    postcard::from_bytes(answer).context("could not read the Trezor's answer")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_remembers_each_network_and_slot_apart() {
        let directory = std::env::temp_dir().join(format!("anzen-trezor-{}", std::process::id()));
        let mut memory = Directory(directory.clone());
        let sealed = |byte| Sealed {
            state: vec![byte; 5],
            seal: [byte; 32],
        };
        assert_eq!(memory.load(Network::Regtest, Slot::Vault).unwrap(), None);
        memory
            .store(Network::Regtest, Slot::Vault, Some(&sealed(1)))
            .unwrap();
        memory
            .store(Network::Regtest, Slot::Rotated, Some(&sealed(2)))
            .unwrap();
        memory
            .store(Network::Mainnet, Slot::Vault, Some(&sealed(3)))
            .unwrap();
        assert_eq!(
            memory.load(Network::Regtest, Slot::Vault).unwrap(),
            Some(sealed(1))
        );
        assert_eq!(
            memory.load(Network::Regtest, Slot::Rotated).unwrap(),
            Some(sealed(2))
        );
        assert_eq!(
            memory.load(Network::Mainnet, Slot::Vault).unwrap(),
            Some(sealed(3))
        );
        memory.store(Network::Regtest, Slot::Vault, None).unwrap();
        memory.store(Network::Regtest, Slot::Vault, None).unwrap();
        assert_eq!(memory.load(Network::Regtest, Slot::Vault).unwrap(), None);
        // A damaged file is no memory, not an error.
        std::fs::write(memory.path(Network::Mainnet, Slot::Vault), [0xff]).unwrap();
        assert_eq!(memory.load(Network::Mainnet, Slot::Vault).unwrap(), None);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
