//! The checked-in byte-level graph vector must be reproducible from its high-level parameters.

use anzen_protocol::{
    address::{self, Network},
    crypto::SoftwareCrypto,
    policy::{self, Destinations, Input, Limits, PolicyInputs},
    script::{Scripts, VaultKeys, XOnlyKey},
    spend,
    tx::{OutPoint, SpendPath, Transaction},
};
use serde_json::Value;

fn vector() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test-vectors/vault-output-graph.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The controller descriptor `tr(NUMS,{pk(phone),pk(hww)})` names both keys; the vault's
/// internal key is their aggregate, so it does not.
fn keys_from_descriptor(descriptor: &str) -> VaultKeys {
    let mut keys = descriptor.split("pk(").skip(1).map(|part| &part[..64]);
    VaultKeys {
        phone: XOnlyKey::from_hex(keys.next().unwrap()).unwrap(),
        hww: XOnlyKey::from_hex(keys.next().unwrap()).unwrap(),
    }
}

fn named<'a>(vector: &'a Value, name: &str) -> &'a Value {
    vector["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|transaction| transaction["name"] == name)
        .unwrap()
}

fn assert_matches(expected: &Value, actual: &Transaction, txid: &str, fee: u64) {
    assert_eq!(expected["txid"], txid, "{}", expected["name"]);
    assert_eq!(expected["fee_sats"], fee, "{}", expected["name"]);
    let inputs = expected["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), actual.input.len());
    for (expected, actual) in inputs.iter().zip(&actual.input) {
        assert_eq!(expected["txid"], actual.previous_output.txid.to_string());
        assert_eq!(expected["vout"], actual.previous_output.vout);
        assert_eq!(expected["sequence"], actual.sequence);
    }
    let outputs = expected["outputs"].as_array().unwrap();
    assert_eq!(outputs.len(), actual.output.len());
    for (expected, actual) in outputs.iter().zip(&actual.output) {
        assert_eq!(expected["value_sats"], actual.value);
        assert_eq!(
            expected["script_pubkey"],
            bitcoin::hex::DisplayHex::to_lower_hex_string(&actual.script_pubkey[..])
        );
    }
}

#[test]
fn reconstructs_every_transaction_in_the_vault_output_vector() {
    let vector = vector();
    let mut crypto = SoftwareCrypto::new();
    let keys = keys_from_descriptor(vector["controller"]["descriptor"].as_str().unwrap());
    let scripts = Scripts::new(&mut crypto, &keys).unwrap();
    assert_eq!(
        address::from_script_pubkey(&mut crypto, &scripts.vault.script_pubkey, Network::Regtest)
            .unwrap(),
        vector["vault"]["address"]
    );
    assert_eq!(
        address::from_script_pubkey(
            &mut crypto,
            &scripts.controller.script_pubkey,
            Network::Regtest
        )
        .unwrap(),
        vector["controller"]["address"]
    );

    let rollover = named(&vector, "rollover");
    let vault = [Input {
        outpoint: OutPoint::new(
            rollover["inputs"][0]["txid"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            0,
        ),
        value_sats: vector["scenario"]["input_sats"].as_u64().unwrap(),
    }];
    let limits = Limits {
        monthly_limit_sats: vector["policy"]["monthly_limit_sats"].as_u64().unwrap(),
        emergency_access_limit_sats: vector["policy"]["emergency_access_limit_sats"]
            .as_u64()
            .unwrap(),
    };
    let inputs = PolicyInputs {
        scripts: &scripts,
        vault: &vault,
        controllers: &[],
        limits,
    };
    let plan = policy::plan(&inputs).unwrap();
    let hot = |name: &str| {
        address::taproot_script_pubkey(
            &mut SoftwareCrypto::new(),
            named(&vector, name)["outputs"][0]["address"]
                .as_str()
                .unwrap(),
            Network::Regtest,
        )
        .unwrap()
    };
    let destinations = Destinations {
        emergency: Some(hot("emergency:withdrawal")),
        allowances: (1..=plan.allowance_count)
            .map(|step| hot(&format!("allowance:step-{step}:authorization")))
            .collect(),
    };
    let graph = policy::build(&mut crypto, &inputs, &destinations).unwrap();
    assert_eq!(graph.allowances.len(), 3);

    assert_matches(
        rollover,
        &graph.rollover.transaction,
        &graph.rollover.txid.to_string(),
        graph.rollover.fee_sats,
    );
    for step in &graph.allowances {
        let expected = named(
            &vector,
            &format!("allowance:step-{}:authorization", step.step),
        );
        assert_matches(
            expected,
            &step.authorization.transaction,
            &step.authorization.txid.to_string(),
            step.authorization.fee_sats,
        );
    }
    let emergency = graph.emergency.as_ref().unwrap();
    assert_matches(
        named(&vector, "emergency:trigger"),
        &emergency.trigger.transaction,
        &emergency.trigger.txid.to_string(),
        emergency.trigger.fee_sats,
    );
    assert_matches(
        named(&vector, "emergency:withdrawal"),
        &emergency.withdrawal.transaction,
        &emergency.withdrawal.txid.to_string(),
        emergency.withdrawal.fee_sats,
    );

    let revocation = named(&vector, "dynamic-revocation:monthly-state-1");
    let change = address::script_pubkey(
        &mut crypto,
        revocation["outputs"][0]["address"].as_str().unwrap(),
        Network::Regtest,
    )
    .unwrap();
    let spend = spend::controller_revocation(
        &mut crypto,
        &scripts,
        &[Input {
            outpoint: OutPoint::new(graph.rollover.txid, graph.monthly_connector_vout.unwrap()),
            value_sats: 1_000,
        }],
        SpendPath::ControllerPhone,
        &change,
    )
    .unwrap();
    assert_matches(
        revocation,
        &spend.transaction,
        &spend.txid.to_string(),
        spend.fee_sats,
    );
}
