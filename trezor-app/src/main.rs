//! The Anzen cold signer as a Trezor app.
//!
//! The host sends one request, the Trezor shows what it does, and only an approval on the device
//! signs it ([`anzen_signer::hardware::stateless`] defines the protocol). Every decision is
//! [`anzen_signer::Signer`]'s, the same code every other Anzen signer runs. This crate supplies
//! what is the device's own: its key and its screen.
//!
//! It stores nothing, because a Trezor app has no storage. What the signer must remember goes
//! back to the host under the signer's seal, and comes back with the next request.
//!
//! It needs one thing stock Trezor firmware does not give an app: the private key of its vault
//! path (`crypto::get_private_key`, a patch on the firmware this is built against). Core has no
//! BIP340 or MuSig2 call, so the app signs in its own arithmetic.
//!
//! Proof of concept: it runs in the Trezor emulator, and has run once on a Safe 7.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

mod screen;

use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use anzen_protocol::{
    address::{self, Network},
    script::Scripts,
    wire::{
        CooperativeSignature, OfflineActionRequest, PolicyRequest, RotationRequest, SweepRequest,
    },
};
use anzen_signer::{
    CooperativeSession, Keys, Signer, State,
    hardware::{
        Operation, Pair, network_from_code, review,
        stateless::{
            self, Call, MAX_INPUTS, MAX_REQUEST, Outcome, PIECE, PROTOCOL_VERSION, Sealed, answer,
            message,
        },
    },
    software::{Place, SoftwareArithmetic, SoftwareKey},
};
use serde::{Serialize, de::DeserializeOwned};
use trezor_app_sdk::{self as sdk, crypto, wire_error, wire_receive, wire_send};

/// What this device calls itself in the messages a person reads.
const DEVICE: &str = "Trezor";

/// Why a request was not answered with what it asked for.
enum Stop {
    /// The person rejected the review.
    Rejected,
    /// The request is not acceptable; the message goes back to the host.
    Refused(String),
    /// Core failed the app, or asked it to stop.
    Core(sdk::Error),
}

type Result<T> = core::result::Result<T, Stop>;

fn refuse(message: &str) -> Stop {
    Stop::Refused(message.to_string())
}

impl From<anzen_protocol::Error> for Stop {
    fn from(error: anzen_protocol::Error) -> Self {
        Self::Refused(error.to_string())
    }
}

impl From<sdk::Error> for Stop {
    fn from(error: sdk::Error) -> Self {
        Self::Core(error)
    }
}

/// Core starts the app anew for each request: answer one, and exit. A request or an answer
/// too long for one message travels in pieces, and the app stays up between them.
#[unsafe(no_mangle)]
pub fn app() -> sdk::Result<()> {
    let (mut id, mut data) = wire_receive()?;
    while id == message::PART {
        let (next, piece) = {
            wire_send(&[], answer::MORE)?;
            wire_receive()?
        };
        if data.len() + piece.len() > MAX_REQUEST {
            return wire_send(
                b"The request is too large for this device.",
                answer::REFUSED,
            );
        }
        data.extend_from_slice(&piece);
        id = next;
    }
    let (id, data) = match handle(id, data) {
        Ok(data) => (answer::OK, data),
        Err(Stop::Rejected) => (answer::REJECTED, Vec::new()),
        Err(Stop::Refused(message)) => (answer::REFUSED, message.into_bytes()),
        // Asked to stop: Core expects the error itself, and no answer.
        Err(Stop::Core(error)) if error.is_stopped() => return Err(error),
        Err(Stop::Core(error)) => return wire_error(&error),
    };
    let mut rest = data.as_slice();
    while rest.len() > PIECE {
        let (piece, after) = rest.split_at(PIECE);
        wire_send(piece, answer::PART)?;
        // Anything but a request for the next piece abandons the answer.
        if wire_receive()?.0 != message::NEXT {
            return wire_send(b"The answer was not collected.", answer::REFUSED);
        }
        rest = after;
    }
    wire_send(rest, id)
}

fn handle(id: u16, data: Vec<u8>) -> Result<Vec<u8>> {
    match id {
        message::INFO => Ok(vec![PROTOCOL_VERSION]),
        message::IDENTITY => Ok(vault_key(network(&data)?)?.vault_key()?.0.to_vec()),
        id => {
            let operation = stateless::operation(id).ok_or_else(|| {
                refuse("The Anzen app on this Trezor does not know this request.")
            })?;
            let call: Call = decode(&data)?;
            drop(data);
            encode(&run(operation, call)?)
        }
    }
}

fn run(operation: Operation, call: Call) -> Result<Outcome> {
    match operation {
        Operation::Pair => pair(call),
        Operation::Policy => policy(call),
        Operation::Sweep => sweep(call),
        Operation::OfflineAction => offline_action(call),
        Operation::Rotation => rotation(call),
        Operation::AdoptRotation => adopt_rotation(call),
        Operation::ForgetPolicy => forget_policy(call),
        // Not a message: `stateless::operation` never returns it.
        Operation::ForgetVault => Err(refuse("This Trezor stores no vault to forget.")),
    }
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    postcard::from_bytes(bytes).map_err(|_| refuse("This Trezor could not read the request."))
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    postcard::to_allocvec(value).map_err(|_| refuse("The answer is too large for this Trezor."))
}

/// A request with no body of its own is one byte: its network's code.
fn network(data: &[u8]) -> Result<Network> {
    match data {
        [code] => network_from_code(*code),
        _ => None,
    }
    .ok_or_else(|| refuse("This Trezor does not know that network."))
}

/// The signer's vault key for one network: `m/86'/coin'/100'/0/0`, the path every Anzen signer
/// uses. Core releases it because the app's manifest names the path.
fn vault_key(network: Network) -> Result<SoftwareKey> {
    const HARDENED: u32 = 0x8000_0000;
    let coin = match network {
        Network::Mainnet => 0,
        Network::Mutinynet | Network::Regtest => 1,
    };
    let path = [HARDENED | 86, HARDENED | coin, HARDENED | 100, 0, 0];
    let secret = crypto::get_private_key(crypto::Curve::Secp256k1, &path)?;
    Ok(SoftwareKey::new(secret)?)
}

/// State the host carried, if it bears this signer's seal for `place` and is for `network`.
fn open(
    key: &SoftwareKey,
    place: Place,
    carried: Option<Sealed>,
    network: Network,
) -> Result<State> {
    let sealed = carried.ok_or_else(|| match place {
        Place::Vault => refuse("Set up a vault with this Trezor first."),
        Place::Rotated => refuse("No approved rotation is waiting for this Trezor."),
    })?;
    if !key.sealed(place, &sealed.state, &sealed.seal) {
        return Err(refuse("This vault was not set up with this Trezor."));
    }
    let state: State = decode(&sealed.state)?;
    if state.network != network {
        return Err(refuse("This vault is on another network."));
    }
    Ok(state)
}

fn seal(key: &SoftwareKey, place: Place, state: &State) -> Result<Option<Sealed>> {
    let state = encode(state)?;
    let seal = key.seal(place, &state);
    Ok(Some(Sealed { state, seal }))
}

fn check_inputs(inputs: usize) -> Result<()> {
    if inputs > MAX_INPUTS {
        return Err(refuse(
            "Too many inputs for this device. Combine some vault deposits first.",
        ));
    }
    Ok(())
}

fn vault_address(state: &State) -> Result<String> {
    let scripts = Scripts::new(&mut SoftwareArithmetic, &state.keys)?;
    Ok(address::from_script_pubkey(
        &mut SoftwareArithmetic,
        &scripts.vault.script_pubkey,
        state.network,
    )?)
}

/// The vault key while it signs: the screen shows how far it has got. Core also stops an app it
/// has not heard from for 20 seconds, and each signature's report is something to hear.
struct Signing<'a> {
    key: &'a mut SoftwareKey,
    signed: usize,
    total: usize,
    /// Core's failure, kept for after the signer's own error type has passed it by.
    core: Option<sdk::Error>,
}

impl<'a> Signing<'a> {
    fn begin(key: &'a mut SoftwareKey, total: usize) -> Result<Self> {
        screen::signing(0, total)?;
        Ok(Self {
            key,
            signed: 0,
            total,
            core: None,
        })
    }

    fn report(&mut self) -> anzen_protocol::Result<()> {
        self.signed += 1;
        screen::signing(self.signed, self.total).map_err(|error| {
            self.core = Some(error);
            anzen_protocol::Error::SigningFailed
        })
    }

    /// The signer's result, or Core's failure if that is what stopped it.
    fn finish<T>(self, result: anzen_protocol::Result<T>) -> Result<T> {
        if let Some(error) = self.core {
            return Err(Stop::Core(error));
        }
        screen::signed()?;
        Ok(result?)
    }
}

impl Keys for Signing<'_> {
    fn vault_key(&mut self) -> anzen_protocol::Result<anzen_protocol::script::XOnlyKey> {
        self.key.vault_key()
    }

    fn sign(&mut self, message: &[u8; 32]) -> anzen_protocol::Result<[u8; 64]> {
        let signature = self.key.sign(message)?;
        self.report()?;
        Ok(signature)
    }

    fn sign_cooperative(
        &mut self,
        session: &CooperativeSession<'_>,
    ) -> anzen_protocol::Result<CooperativeSignature> {
        let signature = self.key.sign_cooperative(session)?;
        self.report()?;
        Ok(signature)
    }
}

/// Accept a vault. A signer with storage accepts silently and refuses a second vault; this one
/// has no memory to protect that way, so it asks: every state it seals here starts with no
/// approved policy, and the person should know one was made.
fn pair(call: Call) -> Result<Outcome> {
    let pair: Pair = decode(&call.request)?;
    let mut key = vault_key(pair.network)?;
    let state = Signer::new(&mut SoftwareArithmetic, &mut key).pair(
        pair.network,
        pair.phone,
        pair.recovery_locktime,
    )?;
    // The host already carries this very vault: leave it, and the policy it remembers, as it is.
    if let Some(sealed) = &call.carried
        && key.sealed(Place::Vault, &sealed.state, &sealed.seal)
        && decode::<State>(&sealed.state)
            .is_ok_and(|carried| carried.keys == state.keys && carried.network == state.network)
    {
        return Ok(Outcome {
            carry: None,
            answer: Vec::new(),
        });
    }
    let fields = vec![
        ("Vault", vault_address(&state)?),
        ("Network", network_title(state.network).to_string()),
        (
            "Note",
            "This Trezor will co-sign for this vault with your phone. Setting up approves no spending."
                .to_string(),
        ),
    ];
    screen::review("Set up vault", &fields, "Set up", false, "anzen/pair")?;
    Ok(Outcome {
        carry: seal(&key, Place::Vault, &state)?,
        answer: Vec::new(),
    })
}

fn network_title(network: Network) -> &'static str {
    match network {
        Network::Mainnet => "Bitcoin",
        Network::Mutinynet => "Mutinynet (test)",
        Network::Regtest => "Regtest (test)",
    }
}

fn policy(call: Call) -> Result<Outcome> {
    let request: PolicyRequest = decode(&call.request)?;
    check_inputs(request.vault_inputs.len() + request.controller_inputs.len())?;
    let mut key = vault_key(request.network)?;
    let state = open(&key, Place::Vault, call.carried, request.network)?;
    let review =
        Signer::new(&mut SoftwareArithmetic, &mut key).review_policy(&state, &request, None)?;
    screen::review(
        "Vault policy",
        &review::policy(&review, DEVICE),
        "Hold to approve",
        true,
        "anzen/policy",
    )?;
    let mut signing = Signing::begin(&mut key, request.phone_nonces.len())?;
    let signed = Signer::new(&mut SoftwareArithmetic, &mut signing)
        .sign_policy_request(&state, &request, None);
    let (signatures, next) = signing.finish(signed)?;
    Ok(Outcome {
        carry: seal(&key, Place::Vault, &next)?,
        answer: encode(&signatures)?,
    })
}

fn sweep(call: Call) -> Result<Outcome> {
    let request: SweepRequest = decode(&call.request)?;
    check_inputs(request.inputs.len())?;
    let mut key = vault_key(request.network)?;
    let state = open(&key, Place::Vault, call.carried, request.network)?;
    let review = Signer::new(&mut SoftwareArithmetic, &mut key).review_sweep(&state, &request)?;
    screen::review(
        "Move vault funds",
        &review::sweep(&review),
        "Hold to approve",
        true,
        "anzen/sweep",
    )?;
    let mut signing = Signing::begin(&mut key, request.phone_nonces.len())?;
    let signed =
        Signer::new(&mut SoftwareArithmetic, &mut signing).sign_sweep_request(&state, &request);
    let signatures = signing.finish(signed)?;
    Ok(Outcome {
        carry: None,
        answer: encode(&signatures)?,
    })
}

fn offline_action(call: Call) -> Result<Outcome> {
    let request: OfflineActionRequest = decode(&call.request)?;
    check_inputs(request.inputs.len())?;
    let mut key = vault_key(request.network)?;
    let state = open(&key, Place::Vault, call.carried, request.network)?;
    let review =
        Signer::new(&mut SoftwareArithmetic, &mut key).review_offline_action(&state, &request)?;
    let (title, fields) = review::offline_action(&review);
    screen::review(
        title,
        &fields,
        "Hold to approve",
        true,
        "anzen/offline-action",
    )?;
    let mut signing = Signing::begin(&mut key, request.inputs.len())?;
    let signed = Signer::new(&mut SoftwareArithmetic, &mut signing)
        .sign_offline_action_request(&state, &request);
    let signatures = signing.finish(signed)?;
    Ok(Outcome {
        carry: None,
        answer: encode(&signatures)?,
    })
}

/// Approve moving the vault to a new phone key. The new vault's state goes to the host sealed as
/// a rotation's, which only [`adopt_rotation`] accepts: the old vault stays the signer's vault
/// until the person says the rotation confirmed.
fn rotation(call: Call) -> Result<Outcome> {
    let request: RotationRequest = decode(&call.request)?;
    let renewed = request.renewed_policy.as_ref();
    check_inputs(
        request.sweep_inputs.len()
            + request.revoke_connectors.len()
            + renewed.map_or(0, |policy| {
                policy.vault_inputs.len() + policy.controller_inputs.len()
            }),
    )?;
    let mut key = vault_key(request.network)?;
    let state = open(&key, Place::Vault, call.carried, request.network)?;
    let review =
        Signer::new(&mut SoftwareArithmetic, &mut key).review_rotation(&state, &request, None)?;
    screen::review(
        "Phone rotation",
        &review::rotation(&review),
        "Hold to approve",
        true,
        "anzen/rotation",
    )?;
    let total = request.sweep_nonces.len()
        + request.revoke_connectors.len()
        + renewed.map_or(0, |policy| policy.phone_nonces.len());
    let mut signing = Signing::begin(&mut key, total)?;
    let signed = Signer::new(&mut SoftwareArithmetic, &mut signing)
        .sign_rotation_request(&state, &request, None);
    let (signatures, next) = signing.finish(signed)?;
    Ok(Outcome {
        carry: seal(&key, Place::Rotated, &next)?,
        answer: encode(&signatures)?,
    })
}

/// Make an approved rotation's vault the signer's vault.
fn adopt_rotation(call: Call) -> Result<Outcome> {
    let network = network(&call.request)?;
    let key = vault_key(network)?;
    let rotated = open(&key, Place::Rotated, call.carried, network)?;
    let fields = vec![
        ("New vault", vault_address(&rotated)?),
        (
            "Note",
            "Check on your phone that the rotation confirmed first. This Trezor cannot see the chain."
                .to_string(),
        ),
    ];
    screen::review(
        "Use the rotated vault?",
        &fields,
        "Use it",
        false,
        "anzen/adopt-rotation",
    )?;
    Ok(Outcome {
        carry: seal(&key, Place::Vault, &rotated)?,
        answer: Vec::new(),
    })
}

/// Forget the last approved policy, keeping the vault.
fn forget_policy(call: Call) -> Result<Outcome> {
    let network = network(&call.request)?;
    let key = vault_key(network)?;
    let mut state = open(&key, Place::Vault, call.carried, network)?;
    let fields = vec![
        ("Vault", vault_address(&state)?),
        (
            "Note",
            "Only if your phone never activated the last policy. A new policy will no longer have to replace it."
                .to_string(),
        ),
    ];
    screen::review(
        "Forget the last policy?",
        &fields,
        "Forget it",
        false,
        "anzen/forget-policy",
    )?;
    state.anchor = None;
    state.anchor_approved_at = None;
    Ok(Outcome {
        carry: seal(&key, Place::Vault, &state)?,
        answer: Vec::new(),
    })
}
