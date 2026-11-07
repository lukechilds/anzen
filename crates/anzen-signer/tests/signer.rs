use anzen_protocol::{
    Error,
    address::{self, Network},
    consts::MONTHLY_ALLOWANCE_DELAY_SECONDS,
    crypto::SoftwareCrypto,
    policy::{Input, Limits, Linkage, PolicyGraph},
    script::{Scripts, VaultKeys, XOnlyKey},
    tx::{OutPoint, Txid},
    wire::{
        self, OfflineAction, OfflineActionRequest, PolicyRequest, ReplacementRequest,
        RotationRequest, Signature, SweepRequest,
    },
};
use anzen_signer::{Keys, Signer, State};
use bitcoin::{
    key::{Keypair, Secp256k1},
    secp256k1::{Message, XOnlyPublicKey, rand::thread_rng, schnorr},
};

struct SoftwareKeys(Keypair);

impl SoftwareKeys {
    fn random() -> Self {
        Self(Keypair::new(&Secp256k1::new(), &mut thread_rng()))
    }

    fn xonly(&self) -> XOnlyKey {
        XOnlyKey(self.0.x_only_public_key().0.serialize())
    }
}

impl Keys for SoftwareKeys {
    fn vault_key(&mut self) -> anzen_protocol::Result<XOnlyKey> {
        Ok(self.xonly())
    }

    fn sign(&mut self, message: &[u8; 32]) -> anzen_protocol::Result<[u8; 64]> {
        Ok(Secp256k1::new()
            .sign_schnorr_no_aux_rand(&Message::from_digest(*message), &self.0)
            .serialize())
    }
}

fn verify(key: XOnlyKey, message: [u8; 32], signature: &Signature) {
    Secp256k1::verification_only()
        .verify_schnorr(
            &schnorr::Signature::from_slice(&signature.0).unwrap(),
            &Message::from_digest(message),
            &XOnlyPublicKey::from_slice(&key.0).unwrap(),
        )
        .unwrap();
}

fn txid(byte: u8) -> Txid {
    Txid([byte; 32])
}

fn hot(crypto: &mut SoftwareCrypto, byte: u8) -> String {
    let key = Keypair::from_seckey_slice(&Secp256k1::new(), &[byte; 32]).unwrap();
    let script = anzen_protocol::script::p2tr_script_pubkey(&key.x_only_public_key().0.serialize());
    address::from_script_pubkey(crypto, &script, Network::Regtest).unwrap()
}

struct Fixture {
    crypto: SoftwareCrypto,
    phone: SoftwareKeys,
    hww: SoftwareKeys,
    state: State,
}

fn fixture() -> Fixture {
    let mut crypto = SoftwareCrypto::new();
    let phone = SoftwareKeys::random();
    let mut hww = SoftwareKeys::random();
    let state = Signer::new(&mut crypto, &mut hww)
        .pair(Network::Regtest, phone.xonly())
        .unwrap();
    Fixture {
        crypto,
        phone,
        hww,
        state,
    }
}

impl Fixture {
    fn keys(&self) -> VaultKeys {
        self.state.keys
    }

    fn policy(
        &mut self,
        vault: Vec<Input>,
        controllers: Vec<Input>,
        limits: Limits,
    ) -> PolicyRequest {
        let keys = self.keys();
        let scripts = Scripts::new(&mut self.crypto, &keys).unwrap();
        let plan = anzen_protocol::policy::plan(&anzen_protocol::policy::PolicyInputs {
            scripts: &scripts,
            vault: &vault,
            controllers: &controllers,
            limits,
        })
        .unwrap();
        PolicyRequest {
            version: wire::VERSION,
            network: Network::Regtest,
            keys: self.keys(),
            vault_inputs: vault,
            controller_inputs: controllers,
            limits,
            emergency_destination: (limits.emergency_access_limit_sats > 0)
                .then(|| hot(&mut self.crypto, 99)),
            allowance_destinations: (0..plan.allowance_count)
                .map(|index| hot(&mut self.crypto, index as u8 + 1))
                .collect(),
            previous_allowances_complete: false,
        }
    }

    fn signer(&mut self) -> Signer<'_, SoftwareCrypto, SoftwareKeys> {
        Signer::new(&mut self.crypto, &mut self.hww)
    }
}

const LIMITS: Limits = Limits {
    monthly_limit_sats: 10_000_000,
    emergency_access_limit_sats: 50_000_000,
};

fn vault_input(byte: u8, sats: u64) -> Input {
    Input {
        outpoint: OutPoint::new(txid(byte), 0),
        value_sats: sats,
    }
}

fn graph(fixture: &mut Fixture, request: &PolicyRequest) -> PolicyGraph {
    request.build(&mut fixture.crypto).unwrap().1
}

#[test]
fn policy_approval_signs_every_cooperative_input_and_updates_the_anchor() {
    let mut fixture = fixture();
    let request = fixture.policy(vec![vault_input(1, 200_000_000)], vec![], LIMITS);
    let state = fixture.state.clone();
    let review = fixture
        .signer()
        .review_policy(&state, &request, None)
        .unwrap();
    assert_eq!(review.funded_withdrawals, 12);
    assert_eq!(review.transaction_count, 15);
    assert_eq!(review.previous_policy, Linkage::First);
    assert_eq!(review.allowance_destinations.len(), 12);

    let (approval, next) = fixture
        .signer()
        .approve_policy(&state, &request, Some(1_000))
        .unwrap();
    let graph = graph(&mut fixture, &request);
    let jobs = graph.cooperative_signing_jobs(&mut fixture.crypto).unwrap();
    assert_eq!(approval.signatures.len(), jobs.len());
    for (job, signature) in jobs.iter().zip(&approval.signatures) {
        verify(fixture.hww.xonly(), job.sighash, signature);
    }
    assert_eq!(next.limits, LIMITS);
    assert_eq!(next.anchor, Some(graph.anchor()));
    assert_eq!(next.anchor_approved_at, Some(1_000));
}

#[test]
fn a_renewal_must_consume_or_conflict_with_the_previous_approval() {
    let mut fixture = fixture();
    let first = fixture.policy(vec![vault_input(1, 200_000_000)], vec![], LIMITS);
    let state = fixture.state.clone();
    let (_, approved) = fixture
        .signer()
        .approve_policy(&state, &first, Some(0))
        .unwrap();
    let previous = graph(&mut fixture, &first);

    // A disjoint deposit alone would run a second allowance chain beside the first.
    let parallel = fixture.policy(vec![vault_input(2, 200_000_000)], vec![], LIMITS);
    assert_eq!(
        fixture.signer().review_policy(&approved, &parallel, None),
        Err(Error::UnlinkedPreviousPolicy)
    );

    // Replacing an unbroadcast approval spends the same inputs, so only one can ever confirm.
    let replacement = fixture.policy(
        vec![vault_input(1, 200_000_000), vault_input(2, 200_000_000)],
        vec![],
        LIMITS,
    );
    assert_eq!(
        fixture
            .signer()
            .review_policy(&approved, &replacement, None)
            .unwrap()
            .previous_policy,
        Linkage::ConflictsWithPrevious
    );

    // After activation, the renewal consumes each live chain's vault output or connector.
    let chain = previous.allowances[4].authorization.txid;
    let remainder = OutPoint::new(previous.rollover.txid, previous.remainder_vout);
    let renewal = fixture.policy(
        vec![
            Input {
                outpoint: OutPoint::new(chain, 1),
                value_sats: previous.allowances[5].chain_value_sats,
            },
            Input {
                outpoint: remainder,
                value_sats: previous.plan.remainder_value_sats,
            },
        ],
        vec![Input {
            outpoint: OutPoint::new(chain, 2),
            value_sats: 1_000,
        }],
        LIMITS,
    );
    assert_eq!(
        fixture
            .signer()
            .review_policy(&approved, &renewal, None)
            .unwrap()
            .previous_policy,
        Linkage::ConsumesPrevious
    );

    // With the monthly chain finished only the emergency chain remains; the phone must say so,
    // and a trusted clock rejects the claim before twelve 30-day hops could have elapsed.
    let mut finished = fixture.policy(
        vec![Input {
            outpoint: remainder,
            value_sats: previous.plan.remainder_value_sats,
        }],
        vec![],
        LIMITS,
    );
    assert_eq!(
        fixture.signer().review_policy(&approved, &finished, None),
        Err(Error::UnlinkedPreviousPolicy)
    );
    finished.previous_allowances_complete = true;
    let twelve_months = 12 * u64::from(MONTHLY_ALLOWANCE_DELAY_SECONDS);
    assert_eq!(
        fixture
            .signer()
            .review_policy(&approved, &finished, Some(twelve_months - 1)),
        Err(Error::ImplausibleCompletedAllowances)
    );
    assert_eq!(
        fixture
            .signer()
            .review_policy(&approved, &finished, Some(twelve_months))
            .unwrap()
            .previous_policy,
        Linkage::MonthlyReportedComplete
    );
    // The emergency chain can never be reported finished: its change output always survives.
    let unrelated = fixture.policy(vec![vault_input(9, 200_000_000)], vec![], LIMITS);
    let mut unrelated = unrelated;
    unrelated.previous_allowances_complete = true;
    assert_eq!(
        fixture.signer().review_policy(&approved, &unrelated, None),
        Err(Error::UnlinkedPreviousPolicy)
    );
}

#[test]
fn requests_for_another_vault_or_tampered_destinations_are_rejected() {
    let mut fixture = fixture();
    let state = fixture.state.clone();
    let request = fixture.policy(vec![vault_input(1, 200_000_000)], vec![], LIMITS);

    let mut other_vault = request.clone();
    other_vault.keys.phone = SoftwareKeys::random().xonly();
    assert_eq!(
        fixture.signer().review_policy(&state, &other_vault, None),
        Err(Error::WrongVault)
    );

    let mut other_network = request.clone();
    other_network.network = Network::Mainnet;
    assert_eq!(
        fixture.signer().review_policy(&state, &other_network, None),
        Err(Error::WrongVault)
    );

    let mut missing_step = request.clone();
    missing_step.allowance_destinations.pop();
    assert_eq!(
        fixture.signer().review_policy(&state, &missing_step, None),
        Err(Error::AllowanceCountMismatch)
    );

    let mut non_taproot = request.clone();
    non_taproot.allowance_destinations[0] =
        "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_owned();
    assert_eq!(
        fixture.signer().review_policy(&state, &non_taproot, None),
        Err(Error::UnsupportedDestination)
    );

    let mut other_signer = SoftwareKeys::random();
    assert_eq!(
        Signer::new(&mut fixture.crypto, &mut other_signer).review_policy(&state, &request, None),
        Err(Error::WrongVault)
    );
}

#[test]
fn sweeps_and_offline_actions_sign_each_input_through_the_expected_leaf() {
    let mut fixture = fixture();
    let state = fixture.state.clone();
    let keys = fixture.keys();
    let scripts = Scripts::new(&mut fixture.crypto, &keys).unwrap();
    let destination = hot(&mut fixture.crypto, 50);
    let inputs = vec![vault_input(1, 5_000_000), vault_input(2, 7_000_000)];

    let sweep = SweepRequest {
        version: wire::VERSION,
        network: Network::Regtest,
        keys: fixture.keys(),
        inputs: inputs.clone(),
        destination: destination.clone(),
    };
    let review = fixture.signer().review_sweep(&state, &sweep).unwrap();
    assert_eq!(review.amount_sats + review.fee_sats, 12_000_000);
    let approval = fixture.signer().approve_sweep(&state, &sweep).unwrap();
    let (_, spend) = sweep.build(&mut fixture.crypto).unwrap();
    for (sighash, signature) in spend
        .sighashes(&mut fixture.crypto, &scripts.vault.cooperative_leaf)
        .unwrap()
        .into_iter()
        .zip(&approval.signatures)
    {
        verify(fixture.hww.xonly(), sighash, signature);
    }

    for (action, leaf, inputs) in [
        (
            OfflineAction::HwwRecovery,
            scripts.vault.hww_recovery_leaf,
            inputs.clone(),
        ),
        (
            OfflineAction::RevokeActions,
            scripts.controller.hww_leaf,
            vec![vault_input(3, 1_000), vault_input(4, 1_000)],
        ),
    ] {
        let request = OfflineActionRequest {
            version: wire::VERSION,
            network: Network::Regtest,
            keys: fixture.keys(),
            action,
            inputs,
            destination: destination.clone(),
        };
        let review = fixture
            .signer()
            .review_offline_action(&state, &request)
            .unwrap();
        assert_eq!(
            review.recovery_delay_blocks.is_some(),
            action == OfflineAction::HwwRecovery
        );
        let approval = fixture
            .signer()
            .approve_offline_action(&state, &request)
            .unwrap();
        let (_, spend, _) = request.build(&mut fixture.crypto).unwrap();
        for (sighash, signature) in spend
            .sighashes(&mut fixture.crypto, &leaf)
            .unwrap()
            .into_iter()
            .zip(&approval.signatures)
        {
            verify(fixture.hww.xonly(), sighash, signature);
        }
    }
}

#[test]
fn rotation_preserves_limits_links_the_old_epoch_and_hands_over_new_state() {
    let mut fixture = fixture();
    let first = fixture.policy(vec![vault_input(1, 200_000_000)], vec![], LIMITS);
    let state = fixture.state.clone();
    let (_, approved) = fixture
        .signer()
        .approve_policy(&state, &first, None)
        .unwrap();
    let previous = graph(&mut fixture, &first);
    let new_phone = SoftwareKeys::random().xonly();
    let new_keys = VaultKeys {
        phone: new_phone,
        hww: fixture.keys().hww,
    };
    let sweep_inputs = vec![
        Input {
            outpoint: OutPoint::new(previous.rollover.txid, 0),
            value_sats: previous.plan.allowance_value_sats,
        },
        Input {
            outpoint: OutPoint::new(previous.rollover.txid, 1),
            value_sats: previous.plan.remainder_value_sats,
        },
    ];
    let old_keys = fixture.keys();
    let old_scripts = Scripts::new(&mut fixture.crypto, &old_keys).unwrap();
    let new_scripts = Scripts::new(&mut fixture.crypto, &new_keys).unwrap();
    let sweep = anzen_protocol::spend::vault_sweep(
        &mut fixture.crypto,
        &old_scripts,
        &sweep_inputs,
        anzen_protocol::tx::SpendPath::Cooperative,
        &new_scripts.vault.script_pubkey,
    )
    .unwrap();
    let mut renewed = fixture.policy(
        vec![Input {
            outpoint: OutPoint::new(sweep.txid, 0),
            value_sats: sweep.amount_sats,
        }],
        vec![],
        LIMITS,
    );
    renewed.keys = new_keys;
    let request = RotationRequest {
        version: wire::VERSION,
        network: Network::Regtest,
        old_keys: fixture.keys(),
        new_phone,
        sweep_inputs,
        revoke_connectors: vec![
            Input {
                outpoint: OutPoint::new(previous.rollover.txid, 2),
                value_sats: 1_000,
            },
            Input {
                outpoint: OutPoint::new(previous.rollover.txid, 3),
                value_sats: 1_000,
            },
        ],
        renewed_policy: Some(renewed.clone()),
        previous_allowances_complete: false,
    };
    let review = fixture
        .signer()
        .review_rotation(&approved, &request, None)
        .unwrap();
    assert_eq!(review.previous_policy, Linkage::ConsumesPrevious);
    assert!(review.revokes_connectors);
    assert_eq!(review.funded_withdrawals, 12);

    let (approval, next) = fixture
        .signer()
        .approve_rotation(&approved, &request, Some(5))
        .unwrap();
    assert_eq!(approval.sweep_signatures.len(), 2);
    assert_eq!(approval.revocation_signatures.len(), 2);
    assert_eq!(approval.policy_signatures.len(), 15);
    assert_eq!(next.keys, new_keys);
    assert_eq!(next.limits, LIMITS);
    assert!(next.anchor.is_some());

    let mut changed_limits = request.clone();
    changed_limits.renewed_policy = Some({
        let mut policy = renewed.clone();
        policy.limits.monthly_limit_sats += 1;
        policy
    });
    assert_eq!(
        fixture
            .signer()
            .review_rotation(&approved, &changed_limits, None)
            .err(),
        Some(Error::PolicyLimitsChanged)
    );
    let mut dropped_policy = request.clone();
    dropped_policy.renewed_policy = None;
    assert_eq!(
        fixture
            .signer()
            .review_rotation(&approved, &dropped_policy, None)
            .err(),
        Some(Error::PolicyLimitsChanged)
    );
    let mut detached = request.clone();
    detached.renewed_policy.as_mut().unwrap().vault_inputs[0].value_sats -= 1;
    assert_eq!(
        fixture
            .signer()
            .review_rotation(&approved, &detached, None)
            .err(),
        Some(Error::WrongVault)
    );
}

#[test]
fn a_replacement_signer_renews_only_on_the_phone_recovery_output() {
    let mut crypto = SoftwareCrypto::new();
    let phone = SoftwareKeys::random();
    let old_hww = SoftwareKeys::random();
    let mut new_hww = SoftwareKeys::random();
    let old_keys = VaultKeys {
        phone: phone.xonly(),
        hww: old_hww.xonly(),
    };
    let new_keys = VaultKeys {
        phone: phone.xonly(),
        hww: new_hww.xonly(),
    };
    let recovery_inputs = vec![vault_input(7, 90_000_000)];
    let old_scripts = Scripts::new(&mut crypto, &old_keys).unwrap();
    let new_scripts = Scripts::new(&mut crypto, &new_keys).unwrap();
    let recovery = anzen_protocol::spend::vault_sweep(
        &mut crypto,
        &old_scripts,
        &recovery_inputs,
        anzen_protocol::tx::SpendPath::PhoneRecovery,
        &new_scripts.vault.script_pubkey,
    )
    .unwrap();
    let limits = Limits {
        monthly_limit_sats: 10_000_000,
        emergency_access_limit_sats: 0,
    };
    let vault = vec![Input {
        outpoint: OutPoint::new(recovery.txid, 0),
        value_sats: recovery.amount_sats,
    }];
    let plan = anzen_protocol::policy::plan(&anzen_protocol::policy::PolicyInputs {
        scripts: &new_scripts,
        vault: &vault,
        controllers: &[],
        limits,
    })
    .unwrap();
    let request = ReplacementRequest {
        version: wire::VERSION,
        network: Network::Regtest,
        old_keys,
        new_hww: new_hww.xonly(),
        recovery_inputs,
        renewed_policy: Some(PolicyRequest {
            version: wire::VERSION,
            network: Network::Regtest,
            keys: new_keys,
            vault_inputs: vault,
            controller_inputs: vec![],
            limits,
            emergency_destination: None,
            allowance_destinations: (0..plan.allowance_count)
                .map(|index| hot(&mut crypto, index as u8 + 1))
                .collect(),
            previous_allowances_complete: false,
        }),
    };
    let review = Signer::new(&mut crypto, &mut new_hww)
        .review_replacement(&request, limits)
        .unwrap();
    assert_eq!(review.funded_withdrawals, plan.allowance_count);
    let (approval, state) = Signer::new(&mut crypto, &mut new_hww)
        .approve_replacement(&request, limits, None)
        .unwrap();
    assert_eq!(approval.policy_signatures.len(), plan.allowance_count + 1);
    assert_eq!(state.keys, new_keys);

    // Another device cannot approve on this signer's behalf.
    let mut stranger = SoftwareKeys::random();
    assert_eq!(
        Signer::new(&mut crypto, &mut stranger)
            .review_replacement(&request, limits)
            .err(),
        Some(Error::WrongVault)
    );
    // Limits cannot silently change during replacement.
    assert_eq!(
        Signer::new(&mut crypto, &mut new_hww)
            .review_replacement(
                &request,
                Limits {
                    monthly_limit_sats: 0,
                    emergency_access_limit_sats: 0
                }
            )
            .err(),
        Some(Error::PolicyLimitsChanged)
    );
}

#[test]
fn state_and_requests_round_trip_through_json() {
    let mut fixture = fixture();
    let request = fixture.policy(vec![vault_input(1, 200_000_000)], vec![], LIMITS);
    let text = serde_json::to_string(&request).unwrap();
    assert_eq!(
        serde_json::from_str::<PolicyRequest>(&text).unwrap(),
        request
    );
    let state = fixture.state.clone();
    let (approval, next) = fixture
        .signer()
        .approve_policy(&state, &request, None)
        .unwrap();
    let text = serde_json::to_string(&approval).unwrap();
    assert_eq!(
        serde_json::from_str::<wire::PolicyApproval>(&text).unwrap(),
        approval
    );
    let text = serde_json::to_string(&next).unwrap();
    assert_eq!(serde_json::from_str::<State>(&text).unwrap(), next);
    let _ = &fixture.phone;
}
