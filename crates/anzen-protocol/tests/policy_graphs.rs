//! Graphs pinned across funding shapes and limits. The protocol builder must reproduce every
//! accepted graph byte-for-byte and reject the rest. After an intentional policy change, run with
//! `ANZEN_UPDATE_VECTORS=1` to rewrite the recorded transactions.

use anzen_protocol::{
    address::{self, Network},
    consts::DEFAULT_RECOVERY_LOCKTIME,
    crypto::SoftwareCrypto,
    policy::{self, Destinations, Input, Limits, PolicyInputs},
    script::{Scripts, VaultKeys, XOnlyKey},
};
use serde_json::Value;

fn inputs(values: &Value) -> Vec<Input> {
    values
        .as_array()
        .unwrap()
        .iter()
        .map(|input| Input {
            outpoint: input["outpoint"].as_str().unwrap().parse().unwrap(),
            value_sats: input["value_sats"].as_u64().unwrap(),
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bitcoin::hex::DisplayHex::to_lower_hex_string(bytes)
}

#[test]
fn protocol_reproduces_every_recorded_phone_graph() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test-vectors/policy-graphs.json"
    );
    let mut document: Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let update = std::env::var_os("ANZEN_UPDATE_VECTORS").is_some();
    let mut crypto = SoftwareCrypto::new();
    if update {
        document["keys"]["recovery_locktime"] = DEFAULT_RECOVERY_LOCKTIME.into();
    }
    let keys = VaultKeys {
        phone: XOnlyKey::from_hex(document["keys"]["phone"].as_str().unwrap()).unwrap(),
        hww: XOnlyKey::from_hex(document["keys"]["hww"].as_str().unwrap()).unwrap(),
        recovery_locktime: document["keys"]["recovery_locktime"].as_u64().unwrap() as u32,
    };
    let scripts = Scripts::new(&mut crypto, &keys).unwrap();
    let (mut accepted, mut rejected) = (0, 0);
    for case in document["cases"].as_array_mut().unwrap() {
        let vault = inputs(&case["vault_inputs"]);
        let controllers = inputs(&case["controller_inputs"]);
        let limits: Limits = serde_json::from_value(case["limits"].clone()).unwrap();
        let request = PolicyInputs {
            scripts: &scripts,
            vault: &vault,
            controllers: &controllers,
            limits,
        };
        let outcome = &case["outcome"];
        if outcome.get("error").is_some() {
            assert!(policy::plan(&request).is_err(), "accepted {case}");
            rejected += 1;
            continue;
        }
        let mut script = |text: &Value| {
            address::taproot_script_pubkey(&mut crypto, text.as_str().unwrap(), Network::Regtest)
                .unwrap()
        };
        let destinations = Destinations {
            emergency: (!outcome["emergency_destination"].is_null())
                .then(|| script(&outcome["emergency_destination"])),
            allowances: outcome["allowance_destinations"]
                .as_array()
                .unwrap()
                .iter()
                .map(&mut script)
                .collect(),
        };
        let graph = policy::build(&mut crypto, &request, &destinations).unwrap();
        if update {
            case["outcome"]["transactions"] = graph
                .transactions()
                .iter()
                .map(|actual| {
                    serde_json::json!({
                        "fee_sats": actual.fee_sats,
                        "hex": hex(&actual.transaction.serialize()),
                        "txid": actual.txid.to_string(),
                    })
                })
                .collect();
        }
        let outcome = &case["outcome"];
        let expected = outcome["transactions"].as_array().unwrap();
        let actual = graph.transactions();
        assert_eq!(expected.len(), actual.len());
        for (expected, actual) in expected.iter().zip(actual) {
            assert_eq!(expected["hex"], hex(&actual.transaction.serialize()));
            assert_eq!(expected["txid"], actual.txid.to_string());
            assert_eq!(expected["fee_sats"], actual.fee_sats);
        }
        accepted += 1;
    }
    assert_eq!((accepted, rejected), (80, 64));
    if update {
        let text = serde_json::to_string_pretty(&document).unwrap();
        std::fs::write(path, format!("{text}\n")).unwrap();
    }
}
