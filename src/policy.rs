use crate::{HWW_RECOVERY_BLOCKS, PHONE_RECOVERY_BLOCKS};
use anyhow::{Context, Result};
use bitcoin::{
    Address, Network,
    secp256k1::{Secp256k1, XOnlyPublicKey},
};
use miniscript::{Descriptor, descriptor::DescriptorPublicKey};
use std::str::FromStr;

pub const BIP341_NUMS_KEY: &str =
    "50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";

#[derive(Debug, Clone)]
pub struct VaultPolicy {
    pub descriptor: Descriptor<DescriptorPublicKey>,
    pub address: Address,
}

impl VaultPolicy {
    pub fn new(phone: XOnlyPublicKey, hww: XOnlyPublicKey) -> Result<Self> {
        let descriptor_text = format!(
            "tr({BIP341_NUMS_KEY},{{multi_a(2,{phone},{hww}),{{and_v(v:older({PHONE_RECOVERY_BLOCKS}),pk({phone})),and_v(v:older({HWW_RECOVERY_BLOCKS}),pk({hww}))}}}})"
        );
        let descriptor = Descriptor::<DescriptorPublicKey>::from_str(&descriptor_text)
            .with_context(|| format!("invalid vault descriptor: {descriptor_text}"))?;
        let address = descriptor
            .derived_descriptor(&Secp256k1::verification_only(), 0)?
            .address(Network::Regtest)?;
        Ok(Self {
            descriptor,
            address,
        })
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
        assert!(text.contains("multi_a(2"));
        assert!(text.contains("older(61200)"));
        assert!(text.contains("older(65535)"));
        assert!(text.starts_with("tr(50929b74"));
    }
}
