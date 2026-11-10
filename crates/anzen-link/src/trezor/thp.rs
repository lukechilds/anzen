//! A channel to a Trezor: the Trezor Host Protocol (THP) over any packet transport.
//!
//! THP is Trezor's own encrypted link, the same over USB, Bluetooth and the emulator's UDP
//! socket: a channel is allocated, a Noise handshake keys it, and the host proves it was paired
//! before (or pairs now, by typing the code the Trezor shows). Trezor's `trezor-thp` crate does
//! the packets, the handshake and the encryption; this module drives it over a [`Transport`] and
//! speaks the pairing messages around it.

use super::{
    cpace,
    proto::{self, failure, kind, pairing_method},
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::{sync::mpsc::UnboundedSender, time::Instant};
use trezor_thp::{
    Backend, ChannelIO,
    channel::{
        MAX_RETRANSMISSION_COUNT, PacketInResult, PairingState, Phase,
        buffered::Buffered,
        host::{Channel, Mux},
        retransmit_after_ms,
    },
    credential::{CredentialStore, FoundCredential},
    error::TransportError,
};

/// Messages that never wait for a person.
pub const QUICK: Duration = Duration::from_secs(20);
/// A review, or a PIN, waits for the person.
pub const REVIEW: Duration = Duration::from_secs(300);

/// Anything that carries THP packets to a Trezor and back.
pub trait Transport: Send {
    /// The size of every packet on this transport.
    fn packet_len(&self) -> usize;
    fn send(&mut self, packet: &[u8]) -> impl Future<Output = Result<()>> + Send;
    /// The next packet, or `None` if none arrived within `timeout`.
    fn receive(
        &mut self,
        timeout: Duration,
    ) -> impl Future<Output = Result<Option<Vec<u8>>>> + Send;
}

/// The primitives the handshake runs on: X25519, AES-256-GCM and SHA-256, from RustCrypto.
pub struct RustCrypto;

impl Backend for RustCrypto {
    type DH = trezor_noise_rust_crypto::X25519;
    type Cipher = trezor_noise_rust_crypto::Aes256Gcm;
    type Hash = trezor_noise_rust_crypto::Sha256;

    fn random_bytes(dest: &mut [u8]) {
        getrandom::fill(dest).expect("the operating system has randomness");
    }
}

/// Who this host is to the Trezors it has paired with: one long-term key, and the credential
/// each Trezor issued to it. Kept by the host between connections; with it, a paired Trezor
/// accepts the host again without showing a code. It can open a channel to those Trezors, so it
/// is stored like any other secret, but it approves nothing: every signature still needs the
/// person's hand on the device.
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    #[serde(with = "hex::serde")]
    static_key: [u8; 32],
    #[serde(default)]
    trezors: Vec<PairedTrezor>,
}

#[derive(Clone, Serialize, Deserialize)]
struct PairedTrezor {
    #[serde(with = "hex::serde")]
    static_key: [u8; 32],
    #[serde(with = "hex::serde")]
    credential: Vec<u8>,
}

impl Identity {
    pub fn generate() -> Self {
        let mut static_key = [0; 32];
        RustCrypto::random_bytes(&mut static_key);
        Self {
            static_key,
            trezors: Vec::new(),
        }
    }

    fn public_key(&self) -> [u8; 32] {
        x25519_dalek::x25519(self.static_key, x25519_dalek::X25519_BASEPOINT_BYTES)
    }

    fn remember(&mut self, trezor: [u8; 32], credential: Vec<u8>) {
        self.trezors.retain(|paired| paired.static_key != trezor);
        self.trezors.push(PairedTrezor {
            static_key: trezor,
            credential,
        });
    }

    /// Whether any Trezor has paired with this host.
    pub fn has_paired(&self) -> bool {
        !self.trezors.is_empty()
    }
}

impl CredentialStore for Identity {
    /// A Trezor hides its static key in the handshake: it sends
    /// `X25519(SHA-256(static ‖ ephemeral), static)`, which only a host that already knows the
    /// static key can recognise. This always answers with the host's own key, so a Trezor it has
    /// not paired with still sees the key the host is about to ask a credential for.
    fn lookup<'a>(
        &self,
        ephemeral_pubkey: &[u8],
        masked_static_pubkey: &[u8],
        dest: &'a mut [u8],
    ) -> Option<FoundCredential<'a>> {
        let credential = self
            .trezors
            .iter()
            .find(|paired| {
                let mask: [u8; 32] = Sha256::new()
                    .chain_update(paired.static_key)
                    .chain_update(ephemeral_pubkey)
                    .finalize()
                    .into();
                x25519_dalek::x25519(mask, paired.static_key)[..] == *masked_static_pubkey
            })
            // The handshake carries the credential inside a protocol-buffer message.
            .map(|paired| {
                proto::encode(&proto::HandshakeCompletion {
                    host_pairing_credential: Some(paired.credential.clone()),
                })
            })
            .unwrap_or_default();
        let (key, rest) = dest.split_at_mut_checked(32)?;
        key.copy_from_slice(&self.static_key);
        let stored = rest.get_mut(..credential.len())?;
        stored.copy_from_slice(&credential);
        Some(FoundCredential {
            local_static_privkey: (&*key).try_into().ok()?,
            auth_credential: stored,
        })
    }
}

/// The Trezor answered a request with a `Failure`.
#[derive(Debug)]
pub struct Failure {
    pub code: i32,
    pub message: String,
}

impl Failure {
    /// The person cancelled on the device.
    pub fn is_cancelled(&self) -> bool {
        matches!(
            self.code,
            failure::ACTION_CANCELLED | failure::PIN_CANCELLED
        )
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            failure::ACTION_CANCELLED | failure::PIN_CANCELLED => {
                formatter.write_str("cancelled on the Trezor")
            }
            failure::NOT_INITIALIZED => {
                formatter.write_str("This Trezor has no wallet yet. Set it up first.")
            }
            _ if self.message.is_empty() => {
                write!(formatter, "the Trezor refused (code {})", self.code)
            }
            _ => write!(formatter, "the Trezor refused: {}", self.message),
        }
    }
}

impl std::error::Error for Failure {}

enum Event {
    Message(u8, u16, Vec<u8>),
    /// The Trezor allocated the channel that was asked for.
    Channel,
}

async fn flush<T: Transport, C: ChannelIO>(
    transport: &mut T,
    channel: &mut Buffered<C>,
) -> Result<()> {
    while channel.packet_out_ready() {
        let packet = channel
            .packet_out()
            .map_err(|error| anyhow!("could not frame a packet for the Trezor: {error:?}"))?;
        transport.send(&packet).await?;
    }
    Ok(())
}

async fn send<T: Transport, C: ChannelIO>(
    transport: &mut T,
    channel: &mut Buffered<C>,
    session: u8,
    kind: u16,
    message: &[u8],
) -> Result<()> {
    channel
        .message_in(session, kind, message)
        .map_err(|error| anyhow!("the channel to the Trezor is not ready: {error:?}"))?;
    flush(transport, channel).await
}

/// The Trezor's next message. While the last message sent is unacknowledged it is resent, on
/// the protocol's own schedule; once acknowledged, the wait is for the person and the device.
async fn receive<T: Transport, C: ChannelIO>(
    transport: &mut T,
    channel: &mut Buffered<C>,
    timeout: Duration,
) -> Result<Event> {
    let deadline = Instant::now() + timeout;
    let mut retry = 0_u8;
    loop {
        flush(transport, channel).await?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let unacknowledged = !channel.message_in_ready();
        let wait = if unacknowledged {
            remaining.min(Duration::from_millis(retransmit_after_ms(retry).into()))
        } else {
            remaining
        };
        let Some(packet) = transport.receive(wait).await? else {
            if unacknowledged && Instant::now() < deadline && retry < MAX_RETRANSMISSION_COUNT {
                retry += 1;
                channel
                    .message_retransmit()
                    .map_err(|error| anyhow!("could not resend to the Trezor: {error:?}"))?;
                continue;
            }
            bail!("the Trezor did not answer in time");
        };
        match channel.packet_in(&packet) {
            PacketInResult::Accepted {
                message_ready: true,
                ..
            } => {
                let (session, kind, message) = channel
                    .message_out()
                    .map_err(|error| anyhow!("could not read the Trezor's message: {error:?}"))?;
                // Acknowledge it.
                flush(transport, channel).await?;
                return Ok(Event::Message(session, kind, message));
            }
            PacketInResult::ChannelAllocation => {
                channel.message_out().ok();
                return Ok(Event::Channel);
            }
            PacketInResult::TransportError { error } => bail!(
                "{}",
                match error {
                    TransportError::DeviceLocked => "Unlock your Trezor and try again.",
                    TransportError::TransportBusy =>
                        "Your Trezor is busy with another app. Close it and try again.",
                    _ => "The link to your Trezor was reset. Try again.",
                }
            ),
            PacketInResult::Failed { error } => {
                bail!("the channel to the Trezor failed: {error:?}")
            }
            // An acknowledgement, part of a longer message, or a packet that was not for this
            // channel: keep reading.
            _ => {}
        }
    }
}

async fn expect_message<T: Transport, C: ChannelIO>(
    transport: &mut T,
    channel: &mut Buffered<C>,
    timeout: Duration,
) -> Result<(u8, u16, Vec<u8>)> {
    match receive(transport, channel, timeout).await? {
        Event::Message(session, kind, message) => Ok((session, kind, message)),
        Event::Channel => bail!("the Trezor allocated a channel nobody asked for"),
    }
}

/// What this host is called on the Trezor's screens while pairing.
#[derive(Debug, Clone)]
pub struct Host {
    pub host_name: String,
    pub app_name: String,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            host_name: "Phone".to_owned(),
            app_name: "Anzen".to_owned(),
        }
    }
}

/// A pairing in progress: the Trezor is showing a six-digit code.
pub struct CodeEntry {
    challenge: [u8; 32],
    commitment: Vec<u8>,
    trezor_public_key: [u8; 32],
}

/// An open, encrypted channel to a Trezor.
pub struct Trezor<T> {
    transport: T,
    channel: Buffered<Channel<RustCrypto>>,
    properties: proto::DeviceProperties,
    /// What the handshake made of this host: unknown, paired, or paired and free to connect
    /// without a question.
    handshake: PairingState,
    /// The session application messages travel in. 0 has no wallet behind it.
    session: u8,
    reviews: Option<UnboundedSender<String>>,
}

impl<T: Transport> Trezor<T> {
    /// Allocate a channel and run the handshake. A locked Trezor asks for its PIN on its own
    /// screen first. Afterwards the channel is either paired (call [`Self::finish`]) or needs
    /// pairing ([`Self::skip_pairing`], or [`Self::begin_pairing`]).
    pub async fn connect(mut transport: T, identity: &Identity) -> Result<Self> {
        let mut mux = Mux::<RustCrypto>::new();
        mux.request_channel(true);
        let mut mux = Buffered::new(mux);
        mux.set_packet_len(transport.packet_len());
        send(&mut transport, &mut mux, 0, 0, &[]).await?;
        match receive(&mut transport, &mut mux, QUICK).await? {
            Event::Channel => {}
            Event::Message(..) => bail!("the Trezor answered before a channel was open"),
        }
        ensure!(
            mux.channel_alloc_ready(),
            "the Trezor did not allocate a channel"
        );
        let mut open = mux
            .map(|mux| mux.complete(identity.clone()))
            .map_err(|error| anyhow!("could not open a channel to the Trezor: {error:?}"))?;
        let properties: proto::DeviceProperties = proto::decode(open.device_properties())?;
        if let (Some(major), Some(minor)) = (
            properties.protocol_version_major,
            properties.protocol_version_minor,
        ) {
            open.set_device_protocol_version(
                major.try_into().unwrap_or(u8::MAX),
                minor.try_into().unwrap_or(u8::MAX),
            );
        }
        // Two round trips. The first may wait for the person to unlock the Trezor.
        for _ in 0..2 {
            send(&mut transport, &mut open, 0, 0, &[]).await?;
            expect_message(&mut transport, &mut open, REVIEW).await?;
        }
        ensure!(
            open.handshake_done(),
            "the handshake with the Trezor did not finish"
        );
        let channel = open
            .map(|open| open.complete())
            .map_err(|error| anyhow!("the handshake with the Trezor failed: {error:?}"))?;
        let handshake = match channel.phase() {
            Phase::PairingCredential {
                handshake_pairing_state,
            } => handshake_pairing_state,
            Phase::EncryptedTransport => PairingState::PairedAutoconnect,
        };
        Ok(Self {
            transport,
            channel,
            properties,
            handshake,
            session: 0,
            reviews: None,
        })
    }

    /// The Trezor's model, as it names itself (`T3W1` is a Safe 7).
    pub fn model(&self) -> &str {
        self.properties
            .internal_model
            .as_deref()
            .unwrap_or("Trezor")
    }

    /// Whether the handshake recognised this host as already paired.
    pub fn is_paired(&self) -> bool {
        self.handshake != PairingState::Unpaired
    }

    /// Whether the Trezor offers to pair with no code: the emulator and debug firmware do.
    pub fn can_skip_pairing(&self) -> bool {
        self.properties
            .pairing_methods
            .contains(&pairing_method::SKIP)
    }

    /// The channel's id and handshake hash, which the emulator's debug link takes to reveal the
    /// pairing code.
    pub fn pairing_reference(&self) -> ([u8; 2], Vec<u8>) {
        (
            self.channel.channel_id().to_be_bytes(),
            self.channel.handshake_hash().to_vec(),
        )
    }

    /// Have the name of each screen the Trezor raises sent to `reviews`, as it comes up. Tests
    /// use it to press the emulator's buttons at the right moment.
    pub fn watch_reviews(&mut self, reviews: UnboundedSender<String>) {
        self.reviews = Some(reviews);
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    pub fn transport(&mut self) -> &mut T {
        &mut self.transport
    }

    async fn pairing_request(&mut self, host: &Host) -> Result<()> {
        self.exchange(
            0,
            kind::PAIRING_REQUEST,
            &proto::encode(&proto::PairingRequest {
                host_name: Some(host.host_name.clone()),
                app_name: Some(host.app_name.clone()),
            }),
            kind::PAIRING_REQUEST_APPROVED,
            REVIEW,
        )
        .await
        .map(drop)
    }

    /// Pair with no code, where the Trezor allows it. Nothing is remembered: the next connection
    /// pairs again.
    pub async fn skip_pairing(&mut self, host: &Host) -> Result<()> {
        self.pairing_request(host).await?;
        self.exchange(
            0,
            kind::SELECT_METHOD,
            &proto::encode(&proto::SelectMethod {
                selected_pairing_method: Some(pairing_method::SKIP),
            }),
            kind::END_RESPONSE,
            QUICK,
        )
        .await?;
        self.channel.end_pairing();
        Ok(())
    }

    /// Start pairing by code. The Trezor asks the person to allow it, then shows a six-digit code
    /// for them to type into this host ([`Self::finish_pairing`]).
    pub async fn begin_pairing(&mut self, host: &Host) -> Result<CodeEntry> {
        ensure!(
            self.properties
                .pairing_methods
                .contains(&pairing_method::CODE_ENTRY),
            "this Trezor cannot pair by code"
        );
        self.pairing_request(host).await?;
        let commitment = self
            .exchange(
                0,
                kind::SELECT_METHOD,
                &proto::encode(&proto::SelectMethod {
                    selected_pairing_method: Some(pairing_method::CODE_ENTRY),
                }),
                kind::CODE_ENTRY_COMMITMENT,
                QUICK,
            )
            .await?;
        let commitment = proto::bytes(proto::decode(&commitment)?, "commitment")?;
        let mut challenge = [0; 32];
        RustCrypto::random_bytes(&mut challenge);
        let trezor_key = self
            .exchange(
                0,
                kind::CODE_ENTRY_CHALLENGE,
                &proto::encode(&proto::Bytes {
                    value: Some(challenge.to_vec()),
                }),
                kind::CODE_ENTRY_CPACE_TREZOR,
                QUICK,
            )
            .await?;
        let trezor_public_key = proto::bytes(proto::decode(&trezor_key)?, "pairing key")?
            .try_into()
            .map_err(|_| anyhow!("the Trezor's pairing key is not 32 bytes"))?;
        Ok(CodeEntry {
            challenge,
            commitment,
            trezor_public_key,
        })
    }

    /// Finish pairing with the code the Trezor shows, and keep the credential it issues in
    /// `identity` (which the caller then stores). A wrong code ends the pairing: start again.
    pub async fn finish_pairing(
        &mut self,
        entry: CodeEntry,
        code: &str,
        identity: &mut Identity,
    ) -> Result<()> {
        let code = code.trim();
        ensure!(
            code.len() == 6 && code.bytes().all(|digit| digit.is_ascii_digit()),
            "The code is six digits."
        );
        let handshake_hash = self.channel.handshake_hash().to_vec();
        let mut secret = [0; 32];
        RustCrypto::random_bytes(&mut secret);
        let ours = cpace::exchange(
            code.as_bytes(),
            &handshake_hash,
            &entry.trezor_public_key,
            secret,
        );
        let secret = self
            .exchange(
                0,
                kind::CODE_ENTRY_CPACE_HOST_TAG,
                &proto::encode(&proto::CodeEntryCpaceHostTag {
                    cpace_host_public_key: Some(ours.public_key.to_vec()),
                    tag: Some(Sha256::digest(ours.shared_secret).to_vec()),
                }),
                kind::CODE_ENTRY_SECRET,
                QUICK,
            )
            .await
            .context("That code did not match. Start pairing again.")?;
        let secret = proto::bytes(proto::decode(&secret)?, "pairing secret")?;
        // The Trezor committed to its secret before it saw the challenge, and the code follows
        // from both: a device in the middle could not have shown the same code.
        ensure!(
            Sha256::digest(&secret)[..] == entry.commitment[..],
            "The Trezor's pairing answer does not match what it committed to."
        );
        let digest = Sha256::new()
            .chain_update([pairing_method::CODE_ENTRY as u8])
            .chain_update(&handshake_hash)
            .chain_update(&secret)
            .chain_update(entry.challenge)
            .finalize();
        // The digest as a big-endian number, modulo a million.
        let expected = digest.iter().fold(0_u64, |code, byte| {
            (code * 256 + u64::from(*byte)) % 1_000_000
        });
        ensure!(
            code == format!("{expected:06}"),
            "That code did not match. Start pairing again."
        );

        // Ask for a credential, so the next connection needs no code. The Trezor issues only a
        // plain one straight after pairing; [`Self::finish`] trades it for one that connects
        // without a question.
        self.request_credential(identity, false).await?;
        self.end_pairing().await
    }

    /// Ask the Trezor for a credential for this host's key, and keep it in `identity`.
    async fn request_credential(
        &mut self,
        identity: &mut Identity,
        autoconnect: bool,
    ) -> Result<()> {
        let response = self
            .exchange(
                0,
                kind::CREDENTIAL_REQUEST,
                &proto::encode(&proto::CredentialRequest {
                    host_static_public_key: Some(identity.public_key().to_vec()),
                    autoconnect: Some(autoconnect),
                }),
                kind::CREDENTIAL_RESPONSE,
                REVIEW,
            )
            .await?;
        let response: proto::CredentialResponse = proto::decode(&response)?;
        let trezor = response
            .trezor_static_public_key
            .and_then(|key| <[u8; 32]>::try_from(key).ok())
            .context("the Trezor's credential lacks its key")?;
        let credential = response
            .credential
            .context("the Trezor issued no credential")?;
        identity.remember(trezor, credential);
        Ok(())
    }

    /// Leave the pairing phase of a channel the handshake found paired.
    ///
    /// A host holding only the plain credential of its first pairing is asked about on the
    /// Trezor each time it connects. This asks the Trezor, once, to let the host connect by
    /// itself from now on: the person confirms the connection and then that, and the credential
    /// in `identity` is replaced. Returns whether `identity` changed and should be stored again.
    pub async fn finish(&mut self, identity: &mut Identity) -> Result<bool> {
        if self.channel.is_encrypted_transport() {
            return Ok(false);
        }
        let upgrade = self.handshake == PairingState::Paired;
        if upgrade {
            self.request_credential(identity, true).await?;
        }
        self.end_pairing().await?;
        Ok(upgrade)
    }

    async fn end_pairing(&mut self) -> Result<()> {
        self.exchange(0, kind::END_REQUEST, &[], kind::END_RESPONSE, REVIEW)
            .await?;
        self.channel.end_pairing();
        Ok(())
    }

    /// Open a session on the Trezor's standard wallet (no passphrase). Every call after this
    /// travels in it.
    pub async fn open_wallet(&mut self) -> Result<()> {
        let session = 1;
        self.exchange(
            session,
            kind::CREATE_NEW_SESSION,
            &proto::encode(&proto::CreateNewSession {
                passphrase: Some(String::new()),
                on_device: Some(false),
            }),
            kind::SUCCESS,
            REVIEW,
        )
        .await?;
        self.session = session;
        Ok(())
    }

    /// Send one message in the wallet's session and return the Trezor's answer: its kind and
    /// its bytes. Screens the Trezor raises on the way are acknowledged, and a `Failure` is an
    /// error ([`Failure`]).
    pub async fn call(
        &mut self,
        kind: u16,
        message: &[u8],
        timeout: Duration,
    ) -> Result<(u16, Vec<u8>)> {
        self.call_in(self.session, kind, message, timeout).await
    }

    async fn call_in(
        &mut self,
        session: u8,
        kind: u16,
        message: &[u8],
        timeout: Duration,
    ) -> Result<(u16, Vec<u8>)> {
        send(
            &mut self.transport,
            &mut self.channel,
            session,
            kind,
            message,
        )
        .await?;
        loop {
            let (_, answer, body) =
                expect_message(&mut self.transport, &mut self.channel, timeout).await?;
            match answer {
                // A screen is coming up. The Trezor shows it once the request is acknowledged.
                kind::BUTTON_REQUEST => {
                    let request: proto::ButtonRequest = proto::decode(&body)?;
                    send(
                        &mut self.transport,
                        &mut self.channel,
                        session,
                        kind::BUTTON_ACK,
                        &[],
                    )
                    .await?;
                    if let Some(reviews) = &self.reviews {
                        reviews.send(request.name.unwrap_or_default()).ok();
                    }
                }
                kind::FAILURE => {
                    let failure: proto::Failure = proto::decode(&body)?;
                    return Err(Failure {
                        code: failure.code.unwrap_or_default(),
                        message: failure.message.unwrap_or_default(),
                    }
                    .into());
                }
                _ => return Ok((answer, body)),
            }
        }
    }

    /// [`Self::call_in`], for an answer of one known kind.
    async fn exchange(
        &mut self,
        session: u8,
        kind: u16,
        message: &[u8],
        expected: u16,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let (answer, body) = self.call_in(session, kind, message, timeout).await?;
        ensure!(
            answer == expected,
            "the Trezor answered message {kind} with {answer}, not {expected}"
        );
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_recognises_only_the_trezors_it_paired_with() {
        let mut identity = Identity::generate();
        let trezor_secret = [9_u8; 32];
        let trezor = x25519_dalek::x25519(trezor_secret, x25519_dalek::X25519_BASEPOINT_BYTES);
        let ephemeral = [3_u8; 32];
        let mask: [u8; 32] = Sha256::new()
            .chain_update(trezor)
            .chain_update(ephemeral)
            .finalize()
            .into();
        let masked = x25519_dalek::x25519(mask, trezor);

        // Unknown Trezor: the host's key, and no credential.
        let mut buffer = [0_u8; 160];
        let found = identity.lookup(&ephemeral, &masked, &mut buffer).unwrap();
        assert_eq!(*found.local_static_privkey, identity.static_key);
        assert!(found.auth_credential.is_empty());
        assert!(!identity.has_paired());

        identity.remember(trezor, vec![1, 2, 3]);
        let mut buffer = [0_u8; 160];
        let found = identity.lookup(&ephemeral, &masked, &mut buffer).unwrap();
        // Field 1, three bytes: the credential inside its handshake message.
        assert_eq!(found.auth_credential, [0x0a, 3, 1, 2, 3]);

        // Another Trezor's masked key matches nothing.
        let other = x25519_dalek::x25519([5; 32], x25519_dalek::X25519_BASEPOINT_BYTES);
        let mut buffer = [0_u8; 160];
        let found = identity.lookup(&ephemeral, &other, &mut buffer).unwrap();
        assert!(found.auth_credential.is_empty());

        // It survives being stored.
        let stored = serde_json::to_string(&identity).unwrap();
        let restored: Identity = serde_json::from_str(&stored).unwrap();
        assert_eq!(restored.static_key, identity.static_key);
        assert!(restored.has_paired());
    }
}
