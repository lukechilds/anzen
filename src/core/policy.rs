use super::{HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS};
use anyhow::{Context, Result, bail};
use bitcoin::{
    Address, Network, ScriptBuf,
    secp256k1::{Secp256k1, XOnlyPublicKey},
    taproot::{ControlBlock, LeafVersion, TapLeafHash},
};
use miniscript::{
    Descriptor,
    descriptor::{DefiniteDescriptorKey, DescriptorPublicKey},
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const BIP341_NUMS_KEY: &str =
    "50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";

#[derive(Debug, Clone)]
pub struct VaultPolicy {
    pub descriptor: Descriptor<DescriptorPublicKey>,
    pub address: Address,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpendPath {
    Cooperative,
    PhoneRecovery,
    HwwRecovery,
}

#[derive(Debug, Clone)]
pub struct VaultLeaf {
    pub path: SpendPath,
    pub depth: u8,
    pub script: ScriptBuf,
    pub leaf_hash: TapLeafHash,
    pub control_block: ControlBlock,
}

impl VaultPolicy {
    pub fn new(phone: XOnlyPublicKey, hww: XOnlyPublicKey) -> Result<Self> {
        Self::new_for_network(phone, hww, Network::Regtest)
    }

    pub fn new_for_network(
        phone: XOnlyPublicKey,
        hww: XOnlyPublicKey,
        network: Network,
    ) -> Result<Self> {
        let descriptor_text = format!(
            "tr({BIP341_NUMS_KEY},{{multi_a(2,{phone},{hww}),{{and_v(v:older({PHONE_RECOVERY_BLOCKS}),pk({phone})),and_v(v:older({HWW_RECOVERY_BLOCKS}),pk({hww}))}}}})"
        );
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(&descriptor_text)
            .with_context(|| format!("invalid vault descriptor: {descriptor_text}"))?;
        let address = descriptor
            .derived_descriptor(&Secp256k1::verification_only(), 0)?
            .address(network)?;
        Ok(Self {
            descriptor,
            address,
        })
    }

    pub fn from_descriptor(descriptor_text: &str) -> Result<Self> {
        Self::from_descriptor_for_network(descriptor_text, Network::Regtest)
    }

    pub fn from_descriptor_for_network(descriptor_text: &str, network: Network) -> Result<Self> {
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(descriptor_text)
            .with_context(|| format!("invalid vault descriptor: {descriptor_text}"))?;
        let address = descriptor
            .derived_descriptor(&Secp256k1::verification_only(), 0)?
            .address(network)?;
        Ok(Self {
            descriptor,
            address,
        })
    }

    pub fn definite_descriptor(&self) -> Result<Descriptor<DefiniteDescriptorKey>> {
        self.descriptor
            .at_derivation_index(0)
            .context("vault descriptor could not be made definite")
    }

    pub fn leaf(&self, path: SpendPath) -> Result<VaultLeaf> {
        let derived = self
            .descriptor
            .derived_descriptor(&Secp256k1::verification_only(), 0)?;
        let tr = match derived {
            Descriptor::Tr(tr) => tr,
            _ => bail!("vault descriptor is not Taproot"),
        };
        let spend_info = tr.spend_info();
        let target_depth = match path {
            SpendPath::Cooperative => 1,
            SpendPath::PhoneRecovery | SpendPath::HwwRecovery => 2,
        };
        let target_delay = match path {
            SpendPath::Cooperative => None,
            SpendPath::PhoneRecovery => Some(PHONE_RECOVERY_BLOCKS),
            SpendPath::HwwRecovery => Some(HWW_RECOVERY_BLOCKS),
        };

        for (depth, miniscript) in tr.iter_scripts() {
            if depth != target_depth {
                continue;
            }
            let script = miniscript.encode();
            if let Some(delay) = target_delay {
                let delay_script_num = bitcoin::script::Builder::new()
                    .push_int(i64::from(delay))
                    .into_script();
                if !script
                    .as_bytes()
                    .windows(delay_script_num.len())
                    .any(|window| window == delay_script_num.as_bytes())
                {
                    continue;
                }
            }
            let leaf_version = LeafVersion::TapScript;
            let leaf_hash = TapLeafHash::from_script(&script, leaf_version);
            let control_block = spend_info
                .control_block(&(script.clone(), leaf_version))
                .context("vault leaf has no Taproot control block")?;
            return Ok(VaultLeaf {
                path,
                depth,
                script,
                leaf_hash,
                control_block,
            });
        }
        bail!("vault descriptor does not contain the requested {path:?} leaf")
    }

    pub fn descriptor_string(&self) -> String {
        self.descriptor.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keys::DeviceKeys;
    use bitcoin::key::Secp256k1;

    #[test]
    fn descriptor_is_static_and_has_expected_policy() {
        let secp = Secp256k1::new();
        let phone = DeviceKeys::generate(&secp).unwrap();
        let hww = DeviceKeys::generate(&secp).unwrap();
        let first = VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        let second = VaultPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        assert_eq!(first.address, second.address);
        let text = first.descriptor_string();
        assert!(text.contains("multi_a(2"));
        assert!(text.contains("older(61200)"));
        assert!(text.contains("older(65535)"));
        assert!(text.starts_with("tr(50929b74"));
        assert_eq!(first.leaf(SpendPath::Cooperative).unwrap().depth, 1);
        assert_eq!(first.leaf(SpendPath::PhoneRecovery).unwrap().depth, 2);
        assert_eq!(first.leaf(SpendPath::HwwRecovery).unwrap().depth, 2);
        assert_ne!(
            first.leaf(SpendPath::PhoneRecovery).unwrap().leaf_hash,
            first.leaf(SpendPath::HwwRecovery).unwrap().leaf_hash
        );
    }

    #[test]
    fn the_same_script_policy_encodes_for_mainnet() {
        let secp = Secp256k1::new();
        let phone = DeviceKeys::generate_for_network(&secp, Network::Bitcoin).unwrap();
        let hww = DeviceKeys::generate_for_network(&secp, Network::Bitcoin).unwrap();
        let policy =
            VaultPolicy::new_for_network(phone.vault_pubkey, hww.vault_pubkey, Network::Bitcoin)
                .unwrap();
        assert!(policy.address.to_string().starts_with("bc1p"));
    }
}
