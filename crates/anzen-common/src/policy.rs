use super::{HWW_RECOVERY_SEQUENCE, protocol::SoftwareCrypto};
use anyhow::{Context, Result, bail};
use anzen_protocol::script::{VaultKeys, VaultScript, XOnlyKey};
use bitcoin::{
    Address, Network, ScriptBuf,
    key::TweakedPublicKey,
    opcodes::all::OP_CHECKSIG,
    script::Builder,
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

/// How a vault output is spent: jointly through the key path, or by the hardware wallet alone
/// through the only leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpendPath {
    Cooperative,
    HwwRecovery,
}

/// The vault's internal key: the MuSig2 (BIP327) aggregate of both device keys.
pub fn internal_key(phone: XOnlyPublicKey, hww: XOnlyPublicKey) -> Result<XOnlyPublicKey> {
    let keys = VaultKeys {
        phone: XOnlyKey(phone.serialize()),
        hww: XOnlyKey(hww.serialize()),
    };
    let aggregate = anzen_protocol::musig::aggregate_key(&mut SoftwareCrypto::new(), &keys)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(XOnlyPublicKey::from_slice(&aggregate)?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ControllerPath {
    Phone,
    Hww,
}

#[derive(Debug, Clone)]
pub struct VaultLeaf {
    pub path: SpendPath,
    pub depth: u8,
    pub script: ScriptBuf,
    pub leaf_hash: TapLeafHash,
    pub control_block: ControlBlock,
}

#[derive(Debug, Clone)]
pub struct ControllerLeaf {
    pub path: ControllerPath,
    pub depth: u8,
    pub script: ScriptBuf,
    pub leaf_hash: TapLeafHash,
    pub control_block: ControlBlock,
}

/// The fixed 1-of-2 Taproot policy used by every connector UTXO in one vault.
///
/// It deliberately reuses the vault's phone and HWW public keys. Connector state is represented
/// by an outpoint, not by a derived key: every connector is sent to this same address until the
/// vault keys rotate.
#[derive(Debug, Clone)]
pub struct ControllerPolicy {
    pub descriptor: Descriptor<DescriptorPublicKey>,
    pub address: Address,
    phone: XOnlyPublicKey,
    hww: XOnlyPublicKey,
}

/// Vault output keys for many candidate phone keys with one signer, while grinding a vanity
/// address. It uses the protocol's own script construction, so it cannot drift from
/// [`VaultPolicy`]; tests check both anyway.
#[derive(Debug, Clone)]
pub struct VaultAddressTemplate {
    hww_pubkey: XOnlyKey,
}

impl VaultAddressTemplate {
    pub fn new(hww_pubkey: XOnlyPublicKey) -> Result<Self> {
        Ok(Self {
            hww_pubkey: XOnlyKey(hww_pubkey.serialize()),
        })
    }

    pub fn output_key(
        &self,
        crypto: &mut SoftwareCrypto,
        phone_pubkey: XOnlyPublicKey,
    ) -> Result<[u8; 32]> {
        let keys = VaultKeys {
            phone: XOnlyKey(phone_pubkey.serialize()),
            hww: self.hww_pubkey,
        };
        Ok(VaultScript::new(crypto, &keys)
            .map_err(|error| anyhow::anyhow!("{error}"))?
            .output
            .output_key)
    }

    pub fn address(
        &self,
        crypto: &mut SoftwareCrypto,
        phone_pubkey: XOnlyPublicKey,
        network: Network,
    ) -> Result<Address> {
        let output = XOnlyPublicKey::from_slice(&self.output_key(crypto, phone_pubkey)?)?;
        Ok(Address::p2tr_tweaked(
            TweakedPublicKey::dangerous_assume_tweaked(output),
            network,
        ))
    }
}

#[cfg(test)]
fn recovery_script(key: XOnlyPublicKey, sequence: u32) -> ScriptBuf {
    use bitcoin::opcodes::all::{OP_CSV, OP_VERIFY};
    Builder::new()
        .push_int(i64::from(sequence))
        .push_opcode(OP_CSV)
        .push_opcode(OP_VERIFY)
        .push_x_only_key(&key)
        .push_opcode(OP_CHECKSIG)
        .into_script()
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
        let internal = internal_key(phone, hww)?;
        let descriptor_text =
            format!("tr({internal},and_v(v:older({HWW_RECOVERY_SEQUENCE}),pk({hww})))");
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

    /// The hardware wallet's leaf. Joint spends use the key path and have no leaf.
    pub fn leaf(&self, path: SpendPath) -> Result<VaultLeaf> {
        if path == SpendPath::Cooperative {
            bail!("the joint spend uses the vault's key path, not a leaf");
        }
        let derived = self
            .descriptor
            .derived_descriptor(&Secp256k1::verification_only(), 0)?;
        let tr = match derived {
            Descriptor::Tr(tr) => tr,
            _ => bail!("vault descriptor is not Taproot"),
        };
        let spend_info = tr.spend_info();
        for (depth, miniscript) in tr.iter_scripts() {
            let script = miniscript.encode();
            let delay = bitcoin::script::Builder::new()
                .push_int(i64::from(HWW_RECOVERY_SEQUENCE))
                .into_script();
            if !script
                .as_bytes()
                .windows(delay.len())
                .any(|window| window == delay.as_bytes())
            {
                continue;
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

impl ControllerPolicy {
    pub fn new(phone: XOnlyPublicKey, hww: XOnlyPublicKey) -> Result<Self> {
        Self::new_for_network(phone, hww, Network::Regtest)
    }

    pub fn new_for_network(
        phone: XOnlyPublicKey,
        hww: XOnlyPublicKey,
        network: Network,
    ) -> Result<Self> {
        let descriptor_text = format!("tr({BIP341_NUMS_KEY},{{pk({phone}),pk({hww})}})");
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(&descriptor_text)
            .with_context(|| format!("invalid controller descriptor: {descriptor_text}"))?;
        let address = descriptor
            .derived_descriptor(&Secp256k1::verification_only(), 0)?
            .address(network)?;
        Ok(Self {
            descriptor,
            address,
            phone,
            hww,
        })
    }

    pub fn definite_descriptor(&self) -> Result<Descriptor<DefiniteDescriptorKey>> {
        self.descriptor
            .at_derivation_index(0)
            .context("controller descriptor could not be made definite")
    }

    pub fn leaf(&self, path: ControllerPath) -> Result<ControllerLeaf> {
        let key = match path {
            ControllerPath::Phone => self.phone,
            ControllerPath::Hww => self.hww,
        };
        let target_script = Builder::new()
            .push_x_only_key(&key)
            .push_opcode(OP_CHECKSIG)
            .into_script();
        let derived = self
            .descriptor
            .derived_descriptor(&Secp256k1::verification_only(), 0)?;
        let tr = match derived {
            Descriptor::Tr(tr) => tr,
            _ => bail!("controller descriptor is not Taproot"),
        };
        let spend_info = tr.spend_info();
        for (depth, miniscript) in tr.iter_scripts() {
            let script = miniscript.encode();
            if script != target_script {
                continue;
            }
            let leaf_version = LeafVersion::TapScript;
            let leaf_hash = TapLeafHash::from_script(&script, leaf_version);
            let control_block = spend_info
                .control_block(&(script.clone(), leaf_version))
                .context("controller leaf has no Taproot control block")?;
            return Ok(ControllerLeaf {
                path,
                depth,
                script,
                leaf_hash,
                control_block,
            });
        }
        bail!("controller descriptor does not contain the requested {path:?} leaf")
    }

    pub fn descriptor_string(&self) -> String {
        self.descriptor.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::DeviceKeys;
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
        let internal = internal_key(phone.vault_pubkey, hww.vault_pubkey).unwrap();
        assert!(text.starts_with(&format!("tr({internal},and_v(v:older(4259839),pk(")));
        assert!(!text.contains(&phone.vault_pubkey.to_string()));
        assert!(first.leaf(SpendPath::Cooperative).is_err());
        assert_eq!(first.leaf(SpendPath::HwwRecovery).unwrap().depth, 0);
        // Aggregation is symmetric: it sorts the keys first.
        assert_eq!(
            internal,
            internal_key(hww.vault_pubkey, phone.vault_pubkey).unwrap()
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

    #[test]
    fn fast_address_template_matches_the_canonical_miniscript_policy() {
        let secp = Secp256k1::new();
        for network in [Network::Regtest, Network::Bitcoin] {
            let phone = DeviceKeys::generate_for_network(&secp, network).unwrap();
            let hww = DeviceKeys::generate_for_network(&secp, network).unwrap();
            let policy =
                VaultPolicy::new_for_network(phone.vault_pubkey, hww.vault_pubkey, network)
                    .unwrap();
            let template = VaultAddressTemplate::new(hww.vault_pubkey).unwrap();

            assert_eq!(
                template
                    .address(&mut SoftwareCrypto::new(), phone.vault_pubkey, network)
                    .unwrap(),
                policy.address
            );
            assert_eq!(
                recovery_script(hww.vault_pubkey, HWW_RECOVERY_SEQUENCE),
                policy.leaf(SpendPath::HwwRecovery).unwrap().script
            );
        }
    }

    #[test]
    fn controller_reuses_vault_keys_with_two_single_signature_paths() {
        let secp = Secp256k1::new();
        let phone = DeviceKeys::generate(&secp).unwrap();
        let hww = DeviceKeys::generate(&secp).unwrap();
        let controller = ControllerPolicy::new(phone.vault_pubkey, hww.vault_pubkey).unwrap();

        assert!(
            controller
                .descriptor_string()
                .contains(&phone.vault_pubkey.to_string())
        );
        assert!(
            controller
                .descriptor_string()
                .contains(&hww.vault_pubkey.to_string())
        );
        assert_ne!(
            controller.leaf(ControllerPath::Phone).unwrap().leaf_hash,
            controller.leaf(ControllerPath::Hww).unwrap().leaf_hash
        );
        assert_eq!(controller.leaf(ControllerPath::Phone).unwrap().depth, 1);
        assert_eq!(controller.leaf(ControllerPath::Hww).unwrap().depth, 1);
    }
}
