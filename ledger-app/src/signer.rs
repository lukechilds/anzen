//! The signer's commands: the host uploads a request, the device shows what it does, and only an
//! approval on the device signs it ([`anzen_signer::hardware`] defines the protocol).
//!
//! Every decision is [`anzen_signer::Signer`]'s, the same code every other Anzen signer runs.
//! This module supplies what is the device's own: its key, its storage and its screen.
//!
//! Proof of concept: a request is uploaded whole and held in memory while it is reviewed and
//! signed, which is why [`MAX_INPUTS`] exists and why the functions below are shaped around the
//! stack (one never-inlined function per operation, requests and state boxed, the review freed
//! before signing).

use crate::storage::{self, Place};
use crate::{
    APP_ICON,
    bolos::{Bolos, VaultKey},
};
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use anzen_protocol::{
    address::{self, Network},
    policy::Linkage,
    script::Scripts,
    wire::{OfflineAction, OfflineActionRequest, PolicyRequest, RotationRequest, SweepRequest},
};
use anzen_signer::{
    Keys, Signer, State,
    hardware::{self, CHUNK, MAX_INPUTS, MAX_REQUEST, Operation, Pair, ins, status},
};
use ledger_device_sdk::{
    io::Comm,
    nbgl::{
        Field, InfoLongPress, NbglChoice, NbglGenericReview, NbglPageContent, NbglSpinner,
        TagValueList, TuneIndex,
    },
};
use serde::{Serialize, de::DeserializeOwned};

#[inline(never)]
fn load(place: Place) -> Option<Box<State>> {
    postcard::from_bytes(storage::read(place)?)
        .ok()
        .map(Box::new)
}

#[inline(never)]
fn store(place: Place, state: Option<&State>) -> Result<(), Refusal> {
    let state = state.map(postcard::to_allocvec).transpose();
    let state = state.map_err(|_| refuse("This vault has too many inputs for this device."))?;
    storage::write(place, state.as_deref().unwrap_or_default())
        .map_err(|()| refuse("This vault has too many inputs for this device."))
}

/// Why a command did not run.
pub enum Refusal {
    /// The person rejected the review.
    Rejected,
    /// The request is not acceptable; the message goes back to the host.
    Refused(String),
    /// Commands arrived out of order.
    Sequence,
}

fn refuse(message: &str) -> Refusal {
    Refusal::Refused(message.to_string())
}

impl From<anzen_protocol::Error> for Refusal {
    fn from(error: anzen_protocol::Error) -> Self {
        Self::Refused(error.to_string())
    }
}

/// The request being uploaded and the answer being downloaded.
#[derive(Default)]
pub struct Session {
    request: Vec<u8>,
    answer: Vec<u8>,
    sent: usize,
}

/// What to send back, and what the screen needs afterwards.
pub struct Reply {
    pub data: Vec<u8>,
    pub status: u16,
    /// A review or notice covered the home screen.
    pub redraw: bool,
    /// Shown briefly after the reply is sent.
    pub notice: Option<&'static core::ffi::CStr>,
}

impl Session {
    #[inline(never)]
    pub fn handle(
        &mut self,
        comm: &mut Comm,
        instruction: u8,
        p1: u8,
        p2: u8,
        data: &[u8],
    ) -> Reply {
        let mut shown = false;
        let mut notice = None;
        let result = match instruction {
            ins::INFO => Ok(vec![hardware::PROTOCOL_VERSION]),
            ins::IDENTITY => {
                network(p2).and_then(|network| Ok(VaultKey::new(network).vault_key()?.0.to_vec()))
            }
            ins::STATE => network(p2).and_then(|network| {
                let state = load(Place::Vault(network));
                self.begin_download(encode(&state)?)
            }),
            ins::UPLOAD => self.upload(p1, data),
            ins::RUN => {
                shown = true;
                let request = core::mem::take(&mut self.request);
                network(p2)
                    .and_then(|network| {
                        let operation = Operation::from_code(p1).ok_or(Refusal::Sequence)?;
                        run(comm, operation, network, request, &mut notice)
                    })
                    .and_then(|answer| self.begin_download(answer))
            }
            ins::DOWNLOAD => self.download(),
            // Development builds only: stack size, deepest use, use now, largest free heap block.
            #[cfg(feature = "measure")]
            ins::MEMORY => {
                let usage = crate::memory::usage();
                if p1 == 1 {
                    crate::memory::paint();
                }
                Ok(usage
                    .iter()
                    .flat_map(|value| (*value as u16).to_be_bytes())
                    .collect())
            }
            _ => Err(Refusal::Sequence),
        };
        let (data, status) = match result {
            Ok(data) => (data, status::OK),
            Err(Refusal::Rejected) => (Vec::new(), status::REJECTED),
            Err(Refusal::Refused(message)) => {
                let mut message = message.into_bytes();
                message.truncate(CHUNK);
                (message, status::REFUSED)
            }
            Err(Refusal::Sequence) => (Vec::new(), status::BAD_SEQUENCE),
        };
        Reply {
            data,
            status,
            redraw: shown,
            notice,
        }
    }

    fn upload(&mut self, p1: u8, data: &[u8]) -> Result<Vec<u8>, Refusal> {
        match p1 {
            0 => self.request.clear(),
            1 => {}
            _ => return Err(Refusal::Sequence),
        }
        if self.request.len() + data.len() > MAX_REQUEST {
            self.request = Vec::new();
            return Err(refuse("The request is too large for this device."));
        }
        self.answer = Vec::new();
        self.request.extend_from_slice(data);
        Ok(Vec::new())
    }

    /// The answer's length, then its first chunk.
    fn begin_download(&mut self, answer: Vec<u8>) -> Result<Vec<u8>, Refusal> {
        self.answer = answer;
        self.sent = 0;
        let mut first = hardware::download_header(self.answer.len()).to_vec();
        first.extend(self.next_chunk(CHUNK - first.len()));
        Ok(first)
    }

    fn download(&mut self) -> Result<Vec<u8>, Refusal> {
        if self.sent >= self.answer.len() {
            return Err(Refusal::Sequence);
        }
        Ok(self.next_chunk(CHUNK))
    }

    fn next_chunk(&mut self, most: usize) -> Vec<u8> {
        let end = self.answer.len().min(self.sent + most);
        let chunk = self.answer[self.sent..end].to_vec();
        self.sent = end;
        if self.sent == self.answer.len() {
            self.answer = Vec::new();
            self.sent = 0;
        }
        chunk
    }
}

fn network(code: u8) -> Result<Network, Refusal> {
    hardware::network_from_code(code).ok_or(Refusal::Sequence)
}

/// Read the request, and free its raw bytes before anything is built from it. Boxed, and never
/// inlined, so the request's fields are not on the stack while it is reviewed and signed.
#[inline(never)]
fn decode<T: DeserializeOwned>(request: Vec<u8>) -> Result<Box<T>, Refusal> {
    let decoded = postcard::from_bytes(&request).map(Box::new);
    drop(request);
    crate::memory::report("request decoded");
    decoded.map_err(|_| refuse("This device could not read the request."))
}

#[inline(never)]
fn encode<T: Serialize>(answer: &T) -> Result<Vec<u8>, Refusal> {
    postcard::to_allocvec(answer).map_err(|_| refuse("The answer is too large for this device."))
}

fn accepted(network: Network) -> Result<Box<State>, Refusal> {
    load(Place::Vault(network)).ok_or_else(|| refuse("Set up a vault with this Ledger first."))
}

fn check(requested: Network, network: Network, inputs: usize) -> Result<(), Refusal> {
    if requested != network {
        return Err(Refusal::Sequence);
    }
    if inputs > MAX_INPUTS {
        return Err(refuse(
            "Too many inputs for this device. Combine some vault deposits first.",
        ));
    }
    Ok(())
}

fn vault_address(state: &State) -> Result<String, Refusal> {
    let scripts = Scripts::new(&mut Bolos, &state.keys)?;
    Ok(address::from_script_pubkey(
        &mut Bolos,
        &scripts.vault.script_pubkey,
        state.network,
    )?)
}

type Notice<'a> = &'a mut Option<&'static core::ffi::CStr>;

/// One operation. Each has its own function, never inlined: the stack is small, and one frame
/// holding every operation's locals would use most of it before anything ran.
#[inline(never)]
fn run(
    comm: &mut Comm,
    operation: Operation,
    network: Network,
    request: Vec<u8>,
    notice: Notice<'_>,
) -> Result<Vec<u8>, Refusal> {
    match operation {
        Operation::Pair => pair(network, request, notice),
        Operation::Policy => policy(comm, network, request, notice),
        Operation::Sweep => sweep(comm, network, request, notice),
        Operation::OfflineAction => offline_action(comm, network, request, notice),
        Operation::Rotation => rotation(comm, network, request, notice),
        Operation::AdoptRotation => adopt_rotation(comm, network, notice),
        Operation::ForgetPolicy => forget_policy(comm, network, notice),
        Operation::ForgetVault => forget_vault(comm, network, notice),
    }
}

/// Run `f` with the signer for `network`.
#[inline(never)]
fn with_signer<T>(
    network: Network,
    f: impl FnOnce(&mut Signer<'_, Bolos, VaultKey<'_>>) -> Result<T, Refusal>,
) -> Result<T, Refusal> {
    let (mut crypto, mut keys) = (Bolos, VaultKey::new(network));
    f(&mut Signer::new(&mut crypto, &mut keys))
}

/// [`with_signer`] for signing: the device's events are serviced between signatures.
#[inline(never)]
fn with_signing_signer<T>(
    comm: &mut Comm,
    network: Network,
    f: impl FnOnce(&mut Signer<'_, Bolos, VaultKey<'_>>) -> Result<T, Refusal>,
) -> Result<T, Refusal> {
    let (mut crypto, mut keys) = (Bolos, VaultKey::new(network).serviced_by(comm));
    f(&mut Signer::new(&mut crypto, &mut keys))
}

type Fields = Vec<(&'static str, String)>;

/// Setup asks nothing: accepting a vault approves no spending, and the Bluetooth pairing code
/// already tied this phone to this Ledger. Another vault needs this one forgotten first.
#[inline(never)]
fn pair(network: Network, request: Vec<u8>, notice: Notice<'_>) -> Result<Vec<u8>, Refusal> {
    let pair: Box<Pair> = decode(request)?;
    check(pair.network, network, 0)?;
    let state = with_signer(network, |signer| {
        Ok(Box::new(signer.pair(
            network,
            pair.phone,
            pair.recovery_locktime,
        )?))
    })?;
    match load(Place::Vault(network)) {
        Some(existing) if existing.keys == state.keys => {}
        Some(_) => {
            return Err(refuse(
                "This Ledger already has a vault on this network. Forget it in the Anzen app on the Ledger first.",
            ));
        }
        None => {
            store(Place::Vault(network), Some(&state))?;
            *notice = Some(c"Vault set up");
        }
    }
    Ok(Vec::new())
}

#[inline(never)]
fn policy(
    comm: &mut Comm,
    network: Network,
    request: Vec<u8>,
    notice: Notice<'_>,
) -> Result<Vec<u8>, Refusal> {
    let request: Box<PolicyRequest> = decode(request)?;
    check(
        request.network,
        network,
        request.vault_inputs.len() + request.controller_inputs.len(),
    )?;
    let state = accepted(network)?;
    // The review's graph is freed before the screen is drawn, and built again to sign.
    let fields = policy_fields(network, &state, &request)?;
    crate::memory::report("policy reviewed");
    if !review_fields(comm, fields, "Approve vault policy", "Reject policy") {
        return Err(Refusal::Rejected);
    }
    NbglSpinner::new().show("Signing");
    let (signatures, next) = *with_signing_signer(comm, network, |signer| {
        Ok(Box::new(
            signer.sign_policy_request(&state, &request, None)?,
        ))
    })?;
    crate::memory::report("policy signed");
    drop(request);
    store(Place::Vault(network), Some(&next))?;
    *notice = Some(c"Policy approved");
    encode(&signatures)
}

#[inline(never)]
fn policy_fields(
    network: Network,
    state: &State,
    request: &PolicyRequest,
) -> Result<Fields, Refusal> {
    let review = with_signer(network, |signer| {
        Ok(signer.review_policy(state, request, None)?)
    })?;
    let monthly = if review.monthly_limit_sats > 0 && review.funded_withdrawals > 0 {
        format!(
            "{} a month, {} months funded",
            btc(review.monthly_limit_sats),
            review.funded_withdrawals
        )
    } else {
        "Off".to_string()
    };
    let emergency = match review.emergency_delay_seconds {
        Some(_) => format!("{} after 1 week", btc(review.emergency_access_limit_sats)),
        None => "Off".to_string(),
    };
    let previous = match review.previous_policy {
        Linkage::First => "None: this is the first",
        Linkage::ConflictsWithPrevious | Linkage::ConsumesPrevious => "Replaced by this one",
        Linkage::MonthlyReportedComplete => {
            "Your phone reports its monthly allowances were all used. This Ledger cannot check."
        }
    };
    Ok(vec![
        ("Vault", review.vault_address),
        ("Vault balance", btc(review.vault_input_sats)),
        ("Monthly allowance", monthly),
        ("Emergency access", emergency),
        ("Previous policy", previous.to_string()),
        (
            "Signs",
            format!(
                "{} transactions, {} sats fee",
                review.transaction_count, review.rollover_fee_sats
            ),
        ),
    ])
}

#[inline(never)]
fn sweep(
    comm: &mut Comm,
    network: Network,
    request: Vec<u8>,
    notice: Notice<'_>,
) -> Result<Vec<u8>, Refusal> {
    let request: Box<SweepRequest> = decode(request)?;
    check(request.network, network, request.inputs.len())?;
    let state = accepted(network)?;
    let review = with_signer(network, |signer| Ok(signer.review_sweep(&state, &request)?))?;
    let fields = vec![
        ("Move vault funds", btc(review.amount_sats)),
        ("To", review.destination),
        ("Fee", format!("{} sats", review.fee_sats)),
        ("From vault", review.vault_address),
    ];
    if !review_fields(comm, fields, "Move vault funds now", "Reject transfer") {
        return Err(Refusal::Rejected);
    }
    NbglSpinner::new().show("Signing");
    let signatures = with_signing_signer(comm, network, |signer| {
        Ok(signer.sign_sweep_request(&state, &request)?)
    })?;
    *notice = Some(c"Transfer approved");
    encode(&signatures)
}

#[inline(never)]
fn offline_action(
    comm: &mut Comm,
    network: Network,
    request: Vec<u8>,
    notice: Notice<'_>,
) -> Result<Vec<u8>, Refusal> {
    let request: Box<OfflineActionRequest> = decode(request)?;
    check(request.network, network, request.inputs.len())?;
    let state = accepted(network)?;
    let review = with_signer(network, |signer| {
        Ok(signer.review_offline_action(&state, &request)?)
    })?;
    let (title, note) = match review.action {
        OfflineAction::HwwRecovery => (
            "Recover vault funds",
            "Bitcoin accepts this only about 388 days after each deposit confirmed. Signing does not unlock funds early.",
        ),
        OfflineAction::RevokeActions => (
            "Revoke vault actions",
            "Disables the phone's monthly and emergency withdrawals. Savings stay in the vault.",
        ),
    };
    let fields = vec![
        (title, btc(review.amount_sats)),
        ("To", review.destination),
        ("Fee", format!("{} sats", review.fee_sats)),
        ("From vault", review.vault_address),
        ("Note", note.to_string()),
    ];
    if !review_fields(comm, fields, title, "Reject") {
        return Err(Refusal::Rejected);
    }
    NbglSpinner::new().show("Signing");
    let signatures = with_signing_signer(comm, network, |signer| {
        Ok(signer.sign_offline_action_request(&state, &request)?)
    })?;
    *notice = Some(c"Approved");
    encode(&signatures)
}

#[inline(never)]
fn rotation(
    comm: &mut Comm,
    network: Network,
    request: Vec<u8>,
    notice: Notice<'_>,
) -> Result<Vec<u8>, Refusal> {
    let request: Box<RotationRequest> = decode(request)?;
    let renewed = request.renewed_policy.as_ref().map_or(0, |policy| {
        policy.vault_inputs.len() + policy.controller_inputs.len()
    });
    check(
        request.network,
        network,
        request.sweep_inputs.len() + request.revoke_connectors.len() + renewed,
    )?;
    let state = accepted(network)?;
    let fields = rotation_fields(network, &state, &request)?;
    if !review_fields(comm, fields, "Approve phone rotation", "Reject rotation") {
        return Err(Refusal::Rejected);
    }
    NbglSpinner::new().show("Signing");
    let (signatures, next) = *with_signing_signer(comm, network, |signer| {
        Ok(Box::new(
            signer.sign_rotation_request(&state, &request, None)?,
        ))
    })?;
    drop(request);
    store(Place::Rotated, Some(&next))?;
    *notice = Some(c"Rotation approved");
    encode(&signatures)
}

#[inline(never)]
fn rotation_fields(
    network: Network,
    state: &State,
    request: &RotationRequest,
) -> Result<Fields, Refusal> {
    let review = with_signer(network, |signer| {
        Ok(signer.review_rotation(state, request, None)?)
    })?;
    Ok(vec![
        ("Move to a new phone key", btc(review.amount_sats)),
        ("New phone key", review.new_phone_public_key),
        ("From vault", review.old_vault_address),
        ("To new vault", review.new_vault_address),
        (
            "Policy kept",
            format!(
                "{} a month ({} funded), {} emergency",
                btc(review.monthly_limit_sats),
                review.funded_withdrawals,
                btc(review.emergency_access_limit_sats)
            ),
        ),
        ("Fee", format!("{} sats", review.sweep_fee_sats)),
    ])
}

#[inline(never)]
fn adopt_rotation(
    comm: &mut Comm,
    network: Network,
    notice: Notice<'_>,
) -> Result<Vec<u8>, Refusal> {
    let rotated = load(Place::Rotated)
        .filter(|state| state.network == network)
        .ok_or_else(|| refuse("No approved rotation is waiting on this Ledger."))?;
    let address = vault_address(&rotated)?;
    let approved = NbglChoice::new().glyph(&APP_ICON).show(
        comm,
        "Use the rotated vault?",
        &format!(
            "{address}\n\nCheck on your phone that the rotation confirmed first. This Ledger cannot see the chain."
        ),
        "Use the rotated vault",
        "Keep the previous vault",
    );
    if !approved {
        return Err(Refusal::Rejected);
    }
    store(Place::Vault(network), Some(&rotated))?;
    store(Place::Rotated, None)?;
    *notice = Some(c"Vault updated");
    Ok(Vec::new())
}

#[inline(never)]
fn forget_policy(
    comm: &mut Comm,
    network: Network,
    notice: Notice<'_>,
) -> Result<Vec<u8>, Refusal> {
    let mut state = accepted(network)?;
    let approved = NbglChoice::new().glyph(&APP_ICON).show(
        comm,
        "Forget the last policy?",
        "Only if your phone never activated it. A new policy will no longer have to replace it.",
        "Forget the policy",
        "Keep it",
    );
    if !approved {
        return Err(Refusal::Rejected);
    }
    state.anchor = None;
    state.anchor_approved_at = None;
    store(Place::Vault(network), Some(&state))?;
    *notice = Some(c"Policy forgotten");
    Ok(Vec::new())
}

#[inline(never)]
fn forget_vault(comm: &mut Comm, network: Network, notice: Notice<'_>) -> Result<Vec<u8>, Refusal> {
    let state = accepted(network)?;
    let address = vault_address(&state)?;
    let approved = NbglChoice::new().glyph(&APP_ICON).show(
        comm,
        "Forget this vault?",
        &format!(
            "{address}\n\nThis Ledger keeps its key, so the same phone can set the vault up again. It forgets the last approved policy."
        ),
        "Forget the vault",
        "Keep it",
    );
    if !approved {
        return Err(Refusal::Rejected);
    }
    store(Place::Vault(network), None)?;
    *notice = Some(c"Vault forgotten");
    Ok(Vec::new())
}

/// Bitcoin amounts to at least four decimal places, and as many as the amount needs.
fn btc(sats: u64) -> String {
    let mut text = format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000);
    while text.ends_with('0') && text.len() - text.find('.').expect("has a point") > 5 {
        text.pop();
    }
    text.push_str(" BTC");
    text
}

/// A review: the fields, then a hold-to-approve page. True if the person approved.
#[inline(never)]
fn review_fields(comm: &mut Comm, fields: Fields, approve: &str, reject: &str) -> bool {
    let values = {
        let fields = fields
            .iter()
            .map(|(name, value)| Field { name, value })
            .collect::<Vec<_>>();
        TagValueList::new(&fields, 0, false, true)
    };
    // The screen has its own copy of the text now.
    drop(fields);
    let approval = InfoLongPress::new(
        approve,
        Some(&APP_ICON),
        "Hold to approve",
        TuneIndex::Success,
    );
    NbglGenericReview::new()
        .add_content(NbglPageContent::TagValueList(values))
        .add_content(NbglPageContent::InfoLongPress(approval))
        .show(comm, reject)
}
