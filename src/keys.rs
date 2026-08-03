use anyhow::{Context, Result};
use bdk_wallet::bip39::{Language, Mnemonic};
use bitcoin::{
    Network,
    bip32::{DerivationPath, Xpriv, Xpub},
    key::Secp256k1,
    secp256k1::{All, Keypair, XOnlyPublicKey},
};
use rand::{RngCore, rngs::OsRng};
use std::str::FromStr;

pub const VAULT_DERIVATION_PATH: &str = "m/86'/1'/100'/0/0";
pub const HOT_ACCOUNT_DERIVATION_PATH: &str = "m/86'/1'/0'";

#[derive(Debug, Clone)]
pub struct DeviceKeys {
    pub mnemonic: Mnemonic,
    pub seed: [u8; 64],
    pub master_xpriv: Xpriv,
    pub vault_xpriv: Xpriv,
    pub vault_keypair: Keypair,
    pub vault_pubkey: XOnlyPublicKey,
}

impl DeviceKeys {
    pub fn generate(secp: &Secp256k1<All>) -> Result<Self> {
        let mut entropy = [0_u8; 32];
        OsRng.fill_bytes(&mut entropy);
        let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)
            .context("failed to generate mnemonic")?;
        Self::from_mnemonic(secp, mnemonic)
    }

    pub fn parse(secp: &Secp256k1<All>, words: &str) -> Result<Self> {
        let mnemonic =
            Mnemonic::parse_in_normalized(Language::English, words).context("invalid mnemonic")?;
        Self::from_mnemonic(secp, mnemonic)
    }

    pub fn hot_descriptors(&self, secp: &Secp256k1<All>) -> Result<(String, String)> {
        let account_path = DerivationPath::from_str(HOT_ACCOUNT_DERIVATION_PATH)?;
        let account_xpriv = self.master_xpriv.derive_priv(secp, &account_path)?;
        let account_xpub = Xpub::from_priv(secp, &account_xpriv);
        let fingerprint = self.master_xpriv.fingerprint(secp);
        Ok((
            format!("tr([{fingerprint}/86'/1'/0']{account_xpub}/0/*)"),
            format!("tr([{fingerprint}/86'/1'/0']{account_xpub}/1/*)"),
        ))
    }

    pub fn hot_private_descriptors(&self, secp: &Secp256k1<All>) -> Result<(String, String)> {
        let account_path = DerivationPath::from_str(HOT_ACCOUNT_DERIVATION_PATH)?;
        let account_xpriv = self.master_xpriv.derive_priv(secp, &account_path)?;
        let fingerprint = self.master_xpriv.fingerprint(secp);
        Ok((
            format!("tr([{fingerprint}/86'/1'/0']{account_xpriv}/0/*)"),
            format!("tr([{fingerprint}/86'/1'/0']{account_xpriv}/1/*)"),
        ))
    }

    fn from_mnemonic(secp: &Secp256k1<All>, mnemonic: Mnemonic) -> Result<Self> {
        let seed = mnemonic.to_seed_normalized("");
        let master_xpriv = Xpriv::new_master(Network::Regtest, &seed)?;
        let path = DerivationPath::from_str(VAULT_DERIVATION_PATH)?;
        let vault_xpriv = master_xpriv.derive_priv(secp, &path)?;
        let vault_keypair = Keypair::from_secret_key(secp, &vault_xpriv.private_key);
        let (vault_pubkey, _) = vault_keypair.x_only_public_key();
        Ok(Self {
            mnemonic,
            seed,
            master_xpriv,
            vault_xpriv,
            vault_keypair,
            vault_pubkey,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_recreates_vault_key_and_hot_descriptors() {
        let secp = Secp256k1::new();
        let generated = DeviceKeys::generate(&secp).unwrap();
        let restored = DeviceKeys::parse(&secp, &generated.mnemonic.to_string()).unwrap();
        assert_eq!(generated.vault_pubkey, restored.vault_pubkey);
        assert_eq!(
            generated.hot_descriptors(&secp).unwrap(),
            restored.hot_descriptors(&secp).unwrap()
        );
        assert_eq!(
            generated.hot_private_descriptors(&secp).unwrap(),
            restored.hot_private_descriptors(&secp).unwrap()
        );
    }

    #[test]
    fn vault_key_is_separate_from_hot_account() {
        let secp = Secp256k1::new();
        let keys = DeviceKeys::generate(&secp).unwrap();
        let (external, internal) = keys.hot_descriptors(&secp).unwrap();
        assert_ne!(external, internal);
        assert!(!external.contains(&keys.vault_pubkey.to_string()));
    }
}
