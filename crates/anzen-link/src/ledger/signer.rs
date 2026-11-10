//! The Anzen app on a Ledger, as a signer: each call uploads one request, waits while the person
//! reviews it on the device, and downloads the signatures. Nothing here can approve anything.

use super::{Answer, Exchange, apdu};
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
        CHUNK, CLA, MAX_REQUEST, Operation, PROTOCOL_VERSION, Pair, RotationSignatures, ins,
        network_code, status,
    },
};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

/// The person rejected the review on the device.
#[derive(Debug)]
pub struct Rejected;

impl std::fmt::Display for Rejected {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("rejected on the Ledger")
    }
}

impl std::error::Error for Rejected {}

/// Commands that never wait for a person.
const QUICK: Duration = Duration::from_secs(20);
/// A review waits for the person to read and approve it.
const REVIEW: Duration = Duration::from_secs(300);

pub struct HardwareSigner<T> {
    link: T,
}

impl<T: Exchange> HardwareSigner<T> {
    /// Check the open app is Anzen and speaks this protocol.
    pub async fn open(mut link: T) -> Result<Self> {
        let answer = link
            .exchange(&apdu(CLA, ins::INFO, 0, 0, &[])?, QUICK)
            .await?;
        ensure!(
            answer.status == status::OK,
            "open the Anzen app on your Ledger (it answered {:#06x})",
            answer.status
        );
        ensure!(
            answer.data == [PROTOCOL_VERSION],
            "the Anzen app on your Ledger is a different version: update it"
        );
        Ok(Self { link })
    }

    pub fn into_link(self) -> T {
        self.link
    }

    /// The signer's vault key on `network`.
    pub async fn identity(&mut self, network: Network) -> Result<XOnlyKey> {
        let key = self.command(ins::IDENTITY, 0, network, &[], QUICK).await?;
        Ok(XOnlyKey(
            key.as_slice()
                .try_into()
                .context("the Ledger's key is not 32 bytes")?,
        ))
    }

    /// The vault the device accepted on `network`, and the last policy it approved.
    pub async fn state(&mut self, network: Network) -> Result<Option<State>> {
        let first = self.command(ins::STATE, 0, network, &[], QUICK).await?;
        decode(&self.download(first).await?)
    }

    /// A development build's memory marks: the stack's size, its deepest use since the last
    /// reset, its use now, and the heap's largest free block. `reset` restarts the deepest-use
    /// mark. Release builds refuse the command.
    pub async fn memory(&mut self, reset: bool) -> Result<[u16; 4]> {
        let marks = self
            .command(ins::MEMORY, u8::from(reset), Network::Mainnet, &[], QUICK)
            .await?;
        ensure!(marks.len() == 8, "not a development build of the Anzen app");
        Ok(core::array::from_fn(|index| {
            u16::from_be_bytes([marks[2 * index], marks[2 * index + 1]])
        }))
    }

    /// Accept the vault of this phone key. Asks nothing on the device, and approves no spending.
    pub async fn pair(&mut self, pair: &Pair) -> Result<()> {
        self.run(Operation::Pair, pair.network, Some(&encode(pair)?))
            .await
            .map(drop)
    }

    pub async fn approve_policy(
        &mut self,
        request: &PolicyRequest,
    ) -> Result<Vec<CooperativeSignature>> {
        decode(
            &self
                .run(Operation::Policy, request.network, Some(&encode(request)?))
                .await?,
        )
    }

    pub async fn approve_sweep(
        &mut self,
        request: &SweepRequest,
    ) -> Result<Vec<CooperativeSignature>> {
        decode(
            &self
                .run(Operation::Sweep, request.network, Some(&encode(request)?))
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
                    Some(&encode(request)?),
                )
                .await?,
        )
    }

    pub async fn approve_rotation(
        &mut self,
        request: &RotationRequest,
    ) -> Result<RotationSignatures> {
        decode(
            &self
                .run(
                    Operation::Rotation,
                    request.network,
                    Some(&encode(request)?),
                )
                .await?,
        )
    }

    /// Switch to the vault of the last approved rotation, once it has confirmed.
    pub async fn adopt_rotation(&mut self, network: Network) -> Result<()> {
        self.run(Operation::AdoptRotation, network, None)
            .await
            .map(drop)
    }

    pub async fn forget_policy(&mut self, network: Network) -> Result<()> {
        self.run(Operation::ForgetPolicy, network, None)
            .await
            .map(drop)
    }

    pub async fn forget_vault(&mut self, network: Network) -> Result<()> {
        self.run(Operation::ForgetVault, network, None)
            .await
            .map(drop)
    }

    /// Upload a request, run one operation on it and download its answer.
    async fn run(
        &mut self,
        operation: Operation,
        network: Network,
        request: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        if let Some(request) = request {
            ensure!(
                request.len() <= MAX_REQUEST,
                "the request is too large for a Ledger"
            );
            for (index, chunk) in request.chunks(CHUNK).enumerate() {
                self.command(ins::UPLOAD, u8::from(index > 0), network, chunk, QUICK)
                    .await?;
            }
        }
        let first = self
            .command(ins::RUN, operation as u8, network, &[], REVIEW)
            .await?;
        self.download(first).await
    }

    /// The rest of a download whose first reply (length, then the first chunk) is `first`.
    async fn download(&mut self, first: Vec<u8>) -> Result<Vec<u8>> {
        ensure!(first.len() >= 2, "the Ledger's answer has no length");
        let total = usize::from(u16::from_be_bytes([first[0], first[1]]));
        let mut answer = first[2..].to_vec();
        while answer.len() < total {
            let chunk = self
                .command(ins::DOWNLOAD, 0, Network::Mainnet, &[], QUICK)
                .await?;
            ensure!(!chunk.is_empty(), "the Ledger's answer stopped early");
            answer.extend(chunk);
        }
        ensure!(
            answer.len() == total,
            "the Ledger sent more than it announced"
        );
        Ok(answer)
    }

    async fn command(
        &mut self,
        instruction: u8,
        p1: u8,
        network: Network,
        data: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let command = apdu(CLA, instruction, p1, network_code(network), data)?;
        let Answer { data, status } = self.link.exchange(&command, timeout).await?;
        match status {
            status::OK => Ok(data),
            status::REJECTED => Err(Rejected.into()),
            status::REFUSED => bail!("{}", String::from_utf8_lossy(&data)),
            0x5515 => bail!("unlock your Ledger"),
            0x6e00 | 0x6e01 | 0x6d00 | 0x6d02 => bail!("open the Anzen app on your Ledger"),
            other => bail!("the Ledger answered with status {other:#06x}"),
        }
    }
}

fn encode<M: Serialize>(message: &M) -> Result<Vec<u8>> {
    postcard::to_allocvec(message).context("could not encode the request")
}

fn decode<M: DeserializeOwned>(answer: &[u8]) -> Result<M> {
    postcard::from_bytes(answer).context("could not read the Ledger's answer")
}
