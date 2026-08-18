//! Deterministic, allocation-free benchmark of Anzen's annual signing workload.
//!
//! The benchmark uses fake outpoints and amounts, but constructs the same version-2 Bitcoin
//! transactions and BIP341 script-path signature messages as a real annual policy. Cryptographic
//! operations are supplied by the device integration so Ledger builds can use BOLOS primitives.

use core::cmp::Ordering;

pub const VAULT_BALANCE_SATS: u64 = 210_000_000;
pub const MONTHLY_ALLOWANCE_SATS: u64 = 10_000_000;
pub const EMERGENCY_ACCESS_SATS: u64 = 50_000_000;
pub const MONTHLY_STEPS: usize = 12;
pub const TRANSACTION_COUNT: u8 = 28;
pub const MAX_ROLLOVER_INPUTS: usize = 12;
pub const MAX_SIGNATURE_JOBS: usize = MAX_ROLLOVER_INPUTS + 27;
/// SHA256("Anzen benchmark fixed BIP340 digest v1").
///
/// Hardware integrations sign this same digest repeatedly so signing time is
/// measured independently from transaction-graph and BIP341 hashing time.
pub const FIXED_SIGNING_DIGEST: [u8; 32] = [
    0x56, 0xe2, 0xdd, 0x75, 0xad, 0x8f, 0xc8, 0xc9, 0xf8, 0xf5, 0x8c, 0x2a, 0xdf, 0x74, 0x5f, 0xd3,
    0xf4, 0x66, 0x69, 0xab, 0x38, 0x1c, 0x53, 0x00, 0xa7, 0x1e, 0xb6, 0xa7, 0xa2, 0xac, 0xb5, 0x25,
];
pub const MONTHLY_DELAY_SEQUENCE: u32 = (1 << 22) | 5_063;
pub const EMERGENCY_DELAY_SEQUENCE: u32 = (1 << 22) | 1_182;
pub const PHONE_RECOVERY_BLOCKS: u16 = 61_200;
pub const HWW_RECOVERY_BLOCKS: u16 = 65_535;
pub const BIP341_NUMS_XONLY: [u8; 32] = [
    0x50, 0x92, 0x9b, 0x74, 0xc1, 0xa0, 0x49, 0x54, 0xb7, 0x8b, 0x4b, 0x60, 0x35, 0xe9, 0x7a, 0x5e,
    0x07, 0x8a, 0x5a, 0x0f, 0x28, 0xec, 0x96, 0xd5, 0x47, 0xbf, 0xee, 0x9a, 0xce, 0x80, 0x3a, 0xc0,
];
const SECP256K1_FIELD_PRIME: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xff, 0xff, 0xfc, 0x2f,
];

const MAX_SERIALIZED_TX_SIZE: usize = 640;
const MAX_HASH_COMPONENT_SIZE: usize = 512;
const P2TR_SCRIPT_LEN: usize = 34;
const COOPERATIVE_SCRIPT_LEN: usize = 70;
const COOPERATIVE_CONTROL_BLOCK_LEN: usize = 65;
const TAPLEAF_TAG_HASH: [u8; 32] = [
    0xae, 0xea, 0x8f, 0xdc, 0x42, 0x08, 0x98, 0x31, 0x05, 0x73, 0x4b, 0x58, 0x08, 0x1d, 0x1e, 0x26,
    0x38, 0xd3, 0x5f, 0x1c, 0xb5, 0x40, 0x08, 0xd4, 0xd3, 0x57, 0xca, 0x03, 0xbe, 0x78, 0xe9, 0xee,
];
const TAPBRANCH_TAG_HASH: [u8; 32] = [
    0x19, 0x41, 0xa1, 0xf2, 0xe5, 0x6e, 0xb9, 0x5f, 0xa2, 0xa9, 0xf1, 0x94, 0xbe, 0x5c, 0x01, 0xf7,
    0x21, 0x6f, 0x33, 0xed, 0x82, 0xb0, 0x91, 0x46, 0x34, 0x90, 0xd0, 0x5b, 0xf5, 0x16, 0xa0, 0x15,
];
const TAPTWEAK_TAG_HASH: [u8; 32] = [
    0xe8, 0x0f, 0xe1, 0x63, 0x9c, 0x9c, 0xa0, 0x50, 0xe3, 0xaf, 0x1b, 0x39, 0xc1, 0x43, 0xc6, 0x3e,
    0x42, 0x9c, 0xbc, 0xeb, 0x15, 0xd9, 0x40, 0xfb, 0xb5, 0xc5, 0xa1, 0xf4, 0xaf, 0x57, 0xc5, 0xe9,
];
const TAPSIGHASH_TAG_HASH: [u8; 32] = [
    0xf4, 0x0a, 0x48, 0xdf, 0x4b, 0x2a, 0x70, 0xc8, 0xb4, 0x92, 0x4b, 0xf2, 0x65, 0x46, 0x61, 0xed,
    0x3d, 0x95, 0xfd, 0x66, 0xa3, 0x13, 0xeb, 0x87, 0x23, 0x75, 0x97, 0xc6, 0x28, 0xe4, 0xa0, 0x31,
];

/// SHA-256 implementation supplied by a host test or hardware-wallet integration.
pub trait Sha256 {
    fn hash(&mut self, parts: &[&[u8]]) -> [u8; 32];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchmarkError {
    InvalidRolloverInputCount,
    InvalidPublicKey,
    SerializationOverflow,
    InsufficientValue,
}

/// Normalize a full uncompressed Secp256k1 point to BIP340's implicit even-Y form.
///
/// Generic key-generation APIs return the point corresponding to the secret scalar, which may
/// have odd Y. BIP340 public keys contain only X and always mean the point with even Y.
pub fn normalize_bip340_public_key(mut public_key: [u8; 65]) -> Result<[u8; 65], BenchmarkError> {
    if public_key[0] != 0x04 {
        return Err(BenchmarkError::InvalidPublicKey);
    }
    if public_key[64] & 1 == 1 {
        let mut borrow = 0_i16;
        for index in (0..32).rev() {
            let value = i16::from(SECP256K1_FIELD_PRIME[index])
                - i16::from(public_key[33 + index])
                - borrow;
            if value < 0 {
                public_key[33 + index] = (value + 256) as u8;
                borrow = 1;
            } else {
                public_key[33 + index] = value as u8;
                borrow = 0;
            }
        }
        if borrow != 0 {
            return Err(BenchmarkError::InvalidPublicKey);
        }
    }
    Ok(public_key)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisitError<E> {
    Graph(BenchmarkError),
    Callback(E),
}

impl<E> From<BenchmarkError> for VisitError<E> {
    fn from(value: BenchmarkError) -> Self {
        Self::Graph(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyCommitment {
    pub cooperative_script: [u8; COOPERATIVE_SCRIPT_LEN],
    pub cooperative_leaf_hash: [u8; 32],
    pub merkle_root: [u8; 32],
    pub output_key_tweak: [u8; 32],
}

impl PolicyCommitment {
    pub fn new<H: Sha256>(hasher: &mut H, phone: [u8; 32], hww: [u8; 32]) -> Self {
        let cooperative_script = cooperative_script(phone, hww);
        let cooperative_leaf_hash = tapleaf_hash(hasher, &cooperative_script);
        let phone_recovery = tapleaf_hash(hasher, &recovery_script(phone, PHONE_RECOVERY_BLOCKS));
        let hww_recovery = tapleaf_hash(hasher, &recovery_script(hww, HWW_RECOVERY_BLOCKS));
        let recoveries = tapbranch_hash(hasher, phone_recovery, hww_recovery);
        let merkle_root = tapbranch_hash(hasher, cooperative_leaf_hash, recoveries);
        let output_key_tweak = tagged_hash(
            hasher,
            TAPTWEAK_TAG_HASH,
            &[&BIP341_NUMS_XONLY, &merkle_root],
        );
        Self {
            cooperative_script,
            cooperative_leaf_hash,
            merkle_root,
            output_key_tweak,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TxInput {
    previous_txid: [u8; 32],
    previous_vout: u32,
    amount_sats: u64,
    sequence: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TxOutput {
    amount_sats: u64,
    script_pubkey: [u8; P2TR_SCRIPT_LEN],
}

impl Default for TxOutput {
    fn default() -> Self {
        Self {
            amount_sats: 0,
            script_pubkey: [0_u8; P2TR_SCRIPT_LEN],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BenchmarkTransaction {
    inputs: [TxInput; MAX_ROLLOVER_INPUTS],
    input_count: u8,
    outputs: [TxOutput; 2],
    output_count: u8,
}

impl BenchmarkTransaction {
    fn new(inputs: &[TxInput], outputs: &[TxOutput]) -> Result<Self, BenchmarkError> {
        if inputs.is_empty()
            || inputs.len() > MAX_ROLLOVER_INPUTS
            || outputs.is_empty()
            || outputs.len() > 2
        {
            return Err(BenchmarkError::SerializationOverflow);
        }
        let mut transaction = Self {
            inputs: [TxInput::default(); MAX_ROLLOVER_INPUTS],
            input_count: inputs.len() as u8,
            outputs: [TxOutput::default(); 2],
            output_count: outputs.len() as u8,
        };
        transaction.inputs[..inputs.len()].copy_from_slice(inputs);
        transaction.outputs[..outputs.len()].copy_from_slice(outputs);
        Ok(transaction)
    }

    fn inputs(&self) -> &[TxInput] {
        &self.inputs[..self.input_count as usize]
    }

    fn outputs(&self) -> &[TxOutput] {
        &self.outputs[..self.output_count as usize]
    }

    fn serialize(&self) -> Result<ByteWriter<MAX_SERIALIZED_TX_SIZE>, BenchmarkError> {
        let mut bytes = ByteWriter::new();
        bytes.extend(&2_i32.to_le_bytes())?;
        bytes.push(self.input_count)?;
        for input in self.inputs() {
            bytes.extend(&input.previous_txid)?;
            bytes.extend(&input.previous_vout.to_le_bytes())?;
            bytes.push(0)?;
            bytes.extend(&input.sequence.to_le_bytes())?;
        }
        bytes.push(self.output_count)?;
        serialize_outputs(self.outputs(), &mut bytes)?;
        bytes.extend(&0_u32.to_le_bytes())?;
        Ok(bytes)
    }

    fn txid<H: Sha256>(&self, hasher: &mut H) -> Result<[u8; 32], BenchmarkError> {
        let serialized = self.serialize()?;
        let first = hasher.hash(&[serialized.as_slice()]);
        Ok(hasher.hash(&[&first]))
    }

    fn for_each_sighash<H: Sha256, E, F: FnMut(SignatureJob) -> Result<(), E>>(
        &self,
        hasher: &mut H,
        transaction_index: u8,
        first_signature_index: u8,
        vault_script_pubkey: &[u8; P2TR_SCRIPT_LEN],
        cooperative_leaf_hash: [u8; 32],
        callback: &mut F,
    ) -> Result<u8, VisitError<E>> {
        let mut component = ByteWriter::<MAX_HASH_COMPONENT_SIZE>::new();
        for input in self.inputs() {
            component.extend(&input.previous_txid)?;
            component.extend(&input.previous_vout.to_le_bytes())?;
        }
        let hash_prevouts = hasher.hash(&[component.as_slice()]);

        component.clear();
        for input in self.inputs() {
            component.extend(&input.amount_sats.to_le_bytes())?;
        }
        let hash_amounts = hasher.hash(&[component.as_slice()]);

        component.clear();
        for _ in self.inputs() {
            component.push(P2TR_SCRIPT_LEN as u8)?;
            component.extend(vault_script_pubkey)?;
        }
        let hash_script_pubkeys = hasher.hash(&[component.as_slice()]);

        component.clear();
        for input in self.inputs() {
            component.extend(&input.sequence.to_le_bytes())?;
        }
        let hash_sequences = hasher.hash(&[component.as_slice()]);

        component.clear();
        serialize_outputs(self.outputs(), &mut component)?;
        let hash_outputs = hasher.hash(&[component.as_slice()]);

        let mut next_signature_index = first_signature_index;
        for input_index in 0..self.input_count {
            let mut message = ByteWriter::<256>::new();
            message.push(0)?; // Taproot epoch.
            message.push(0)?; // SIGHASH_DEFAULT.
            message.extend(&2_i32.to_le_bytes())?;
            message.extend(&0_u32.to_le_bytes())?;
            message.extend(&hash_prevouts)?;
            message.extend(&hash_amounts)?;
            message.extend(&hash_script_pubkeys)?;
            message.extend(&hash_sequences)?;
            message.extend(&hash_outputs)?;
            message.push(2)?; // ext_flag=1 (script path), annex absent.
            message.extend(&(input_index as u32).to_le_bytes())?;
            message.extend(&cooperative_leaf_hash)?;
            message.push(0)?; // Tapleaf key version.
            message.extend(&u32::MAX.to_le_bytes())?; // No OP_CODESEPARATOR.
            let sighash = tagged_hash(hasher, TAPSIGHASH_TAG_HASH, &[message.as_slice()]);
            callback(SignatureJob {
                signature_index: next_signature_index,
                transaction_index,
                input_index,
                sighash,
            })
            .map_err(VisitError::Callback)?;
            next_signature_index += 1;
        }
        Ok(next_signature_index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureJob {
    pub signature_index: u8,
    pub transaction_index: u8,
    pub input_index: u8,
    pub sighash: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkloadSummary {
    pub rollover_inputs: u8,
    pub transactions: u8,
    pub signature_jobs: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchmarkConfig {
    rollover_inputs: [TxInput; MAX_ROLLOVER_INPUTS],
    rollover_input_count: u8,
    monthly_hot_scripts: [[u8; P2TR_SCRIPT_LEN]; MONTHLY_STEPS],
    emergency_hot_script: [u8; P2TR_SCRIPT_LEN],
    pub vault_script_pubkey: [u8; P2TR_SCRIPT_LEN],
    pub cooperative_leaf_hash: [u8; 32],
}

impl BenchmarkConfig {
    pub fn deterministic<H: Sha256>(
        hasher: &mut H,
        rollover_input_count: u8,
        vault_output_key: [u8; 32],
        cooperative_leaf_hash: [u8; 32],
    ) -> Result<Self, BenchmarkError> {
        if rollover_input_count == 0 || rollover_input_count as usize > MAX_ROLLOVER_INPUTS {
            return Err(BenchmarkError::InvalidRolloverInputCount);
        }
        let vault_script_pubkey = p2tr_script(vault_output_key);
        let mut rollover_inputs = [TxInput::default(); MAX_ROLLOVER_INPUTS];
        let input_value = VAULT_BALANCE_SATS / u64::from(rollover_input_count);
        let remainder = VAULT_BALANCE_SATS % u64::from(rollover_input_count);
        for (index, input) in rollover_inputs
            .iter_mut()
            .enumerate()
            .take(usize::from(rollover_input_count))
        {
            let index_byte = [index as u8];
            let txid = hasher.hash(&[b"Anzen benchmark fake UTXO v1", &index_byte]);
            *input = TxInput {
                previous_txid: txid,
                previous_vout: 0,
                amount_sats: input_value + u64::from(index == 0) * remainder,
                sequence: u32::MAX,
            };
        }
        let mut monthly_hot_scripts = [[0_u8; P2TR_SCRIPT_LEN]; MONTHLY_STEPS];
        for (index, script) in monthly_hot_scripts.iter_mut().enumerate() {
            let index_byte = [index as u8];
            *script =
                p2tr_script(hasher.hash(&[b"Anzen benchmark monthly hot output v1", &index_byte]));
        }
        let emergency_hot_script =
            p2tr_script(hasher.hash(&[b"Anzen benchmark emergency hot output v1"]));
        Ok(Self {
            rollover_inputs,
            rollover_input_count,
            monthly_hot_scripts,
            emergency_hot_script,
            vault_script_pubkey,
            cooperative_leaf_hash,
        })
    }

    pub fn summary(&self) -> WorkloadSummary {
        WorkloadSummary {
            rollover_inputs: self.rollover_input_count,
            transactions: TRANSACTION_COUNT,
            signature_jobs: self.rollover_input_count + 27,
        }
    }

    pub fn for_each_signature_job<H: Sha256, E, F: FnMut(SignatureJob) -> Result<(), E>>(
        &self,
        hasher: &mut H,
        mut callback: F,
    ) -> Result<WorkloadSummary, VisitError<E>> {
        let mut next_signature = 0_u8;
        let mut transaction_index = 0_u8;
        self.walk_transactions(hasher, |hasher, transaction| {
            next_signature = transaction.for_each_sighash(
                hasher,
                transaction_index,
                next_signature,
                &self.vault_script_pubkey,
                self.cooperative_leaf_hash,
                &mut callback,
            )?;
            transaction_index += 1;
            Ok(())
        })?;
        let summary = self.summary();
        debug_assert_eq!(transaction_index, summary.transactions);
        debug_assert_eq!(next_signature, summary.signature_jobs);
        Ok(summary)
    }

    fn walk_transactions<
        H: Sha256,
        E,
        F: FnMut(&mut H, &BenchmarkTransaction) -> Result<(), VisitError<E>>,
    >(
        &self,
        hasher: &mut H,
        mut callback: F,
    ) -> Result<(), VisitError<E>> {
        let continuing_authorization_fee = cooperative_vsize(1, 2) as u64;
        let final_authorization_fee = cooperative_vsize(1, 1) as u64;
        let allowance_value = MONTHLY_ALLOWANCE_SATS
            .checked_mul(MONTHLY_STEPS as u64)
            .and_then(|value| {
                value.checked_add(continuing_authorization_fee * (MONTHLY_STEPS as u64 - 1))
            })
            .and_then(|value| value.checked_add(final_authorization_fee))
            .ok_or(BenchmarkError::InsufficientValue)?;
        let rollover_fee = cooperative_vsize(self.rollover_input_count, 2) as u64;
        let rollover_remainder = VAULT_BALANCE_SATS
            .checked_sub(allowance_value)
            .and_then(|value| value.checked_sub(rollover_fee))
            .ok_or(BenchmarkError::InsufficientValue)?;
        let rollover = BenchmarkTransaction::new(
            &self.rollover_inputs[..self.rollover_input_count as usize],
            &[
                TxOutput {
                    amount_sats: allowance_value,
                    script_pubkey: self.vault_script_pubkey,
                },
                TxOutput {
                    amount_sats: rollover_remainder,
                    script_pubkey: self.vault_script_pubkey,
                },
            ],
        )?;
        callback(hasher, &rollover)?;
        let rollover_txid = rollover.txid(hasher)?;

        let mut chain_input = TxInput {
            previous_txid: rollover_txid,
            previous_vout: 0,
            amount_sats: allowance_value,
            sequence: MONTHLY_DELAY_SEQUENCE,
        };
        for index in 0..MONTHLY_STEPS {
            let has_next = index + 1 < MONTHLY_STEPS;
            let authorization_fee = if has_next {
                continuing_authorization_fee
            } else {
                final_authorization_fee
            };
            let next_chain_value = chain_input
                .amount_sats
                .checked_sub(MONTHLY_ALLOWANCE_SATS)
                .and_then(|value| value.checked_sub(authorization_fee))
                .ok_or(BenchmarkError::InsufficientValue)?;
            let hot_output = TxOutput {
                amount_sats: MONTHLY_ALLOWANCE_SATS,
                script_pubkey: self.monthly_hot_scripts[index],
            };
            let authorization = if has_next {
                BenchmarkTransaction::new(
                    &[chain_input],
                    &[
                        hot_output,
                        TxOutput {
                            amount_sats: next_chain_value,
                            script_pubkey: self.vault_script_pubkey,
                        },
                    ],
                )?
            } else {
                if next_chain_value != 0 {
                    return Err(BenchmarkError::InsufficientValue.into());
                }
                BenchmarkTransaction::new(&[chain_input], &[hot_output])?
            };
            callback(hasher, &authorization)?;
            let authorization_txid = authorization.txid(hasher)?;

            let revocation_fee = cooperative_vsize(1, 1) as u64;
            let revocation = BenchmarkTransaction::new(
                &[TxInput {
                    sequence: u32::MAX,
                    ..chain_input
                }],
                &[TxOutput {
                    amount_sats: chain_input
                        .amount_sats
                        .checked_sub(revocation_fee)
                        .ok_or(BenchmarkError::InsufficientValue)?,
                    script_pubkey: self.vault_script_pubkey,
                }],
            )?;
            callback(hasher, &revocation)?;

            if has_next {
                chain_input = TxInput {
                    previous_txid: authorization_txid,
                    previous_vout: 1,
                    amount_sats: next_chain_value,
                    sequence: MONTHLY_DELAY_SEQUENCE,
                };
            }
        }

        let withdrawal_fee = cooperative_vsize(1, 1) as u64;
        let staging_value = EMERGENCY_ACCESS_SATS
            .checked_add(withdrawal_fee)
            .ok_or(BenchmarkError::InsufficientValue)?;
        let trigger_fee = cooperative_vsize(1, 2) as u64;
        let vault_change = rollover_remainder
            .checked_sub(staging_value)
            .and_then(|value| value.checked_sub(trigger_fee))
            .ok_or(BenchmarkError::InsufficientValue)?;
        let trigger = BenchmarkTransaction::new(
            &[TxInput {
                previous_txid: rollover_txid,
                previous_vout: 1,
                amount_sats: rollover_remainder,
                sequence: u32::MAX,
            }],
            &[
                TxOutput {
                    amount_sats: staging_value,
                    script_pubkey: self.vault_script_pubkey,
                },
                TxOutput {
                    amount_sats: vault_change,
                    script_pubkey: self.vault_script_pubkey,
                },
            ],
        )?;
        callback(hasher, &trigger)?;
        let trigger_txid = trigger.txid(hasher)?;

        let emergency_input = TxInput {
            previous_txid: trigger_txid,
            previous_vout: 0,
            amount_sats: staging_value,
            sequence: EMERGENCY_DELAY_SEQUENCE,
        };
        let withdrawal = BenchmarkTransaction::new(
            &[emergency_input],
            &[TxOutput {
                amount_sats: EMERGENCY_ACCESS_SATS,
                script_pubkey: self.emergency_hot_script,
            }],
        )?;
        callback(hasher, &withdrawal)?;

        let cancellation = BenchmarkTransaction::new(
            &[TxInput {
                sequence: u32::MAX,
                ..emergency_input
            }],
            &[TxOutput {
                amount_sats: staging_value
                    .checked_sub(cooperative_vsize(1, 1) as u64)
                    .ok_or(BenchmarkError::InsufficientValue)?,
                script_pubkey: self.vault_script_pubkey,
            }],
        )?;
        callback(hasher, &cancellation)?;
        Ok(())
    }
}

pub fn cooperative_script(phone: [u8; 32], hww: [u8; 32]) -> [u8; 70] {
    let mut script = [0_u8; 70];
    script[0] = 32;
    script[1..33].copy_from_slice(&phone);
    script[33] = 0xac; // OP_CHECKSIG
    script[34] = 32;
    script[35..67].copy_from_slice(&hww);
    script[67] = 0xba; // OP_CHECKSIGADD
    script[68] = 0x52; // OP_2
    script[69] = 0x9c; // OP_NUMEQUAL
    script
}

pub fn recovery_script(key: [u8; 32], delay: u16) -> [u8; 40] {
    let mut script = [0_u8; 40];
    script[0] = 3;
    script[1] = delay as u8;
    script[2] = (delay >> 8) as u8;
    script[3] = 0; // Sign bit: both configured delays have their high bit set.
    script[4] = 0xb2; // OP_CHECKSEQUENCEVERIFY
    script[5] = 0x69; // OP_VERIFY
    script[6] = 32;
    script[7..39].copy_from_slice(&key);
    script[39] = 0xac; // OP_CHECKSIG
    script
}

pub fn tapleaf_hash<H: Sha256>(hasher: &mut H, script: &[u8]) -> [u8; 32] {
    debug_assert!(script.len() < 253);
    let leaf_version = [0xc0];
    let script_len = [script.len() as u8];
    tagged_hash(
        hasher,
        TAPLEAF_TAG_HASH,
        &[&leaf_version, &script_len, script],
    )
}

pub fn tapbranch_hash<H: Sha256>(hasher: &mut H, left: [u8; 32], right: [u8; 32]) -> [u8; 32] {
    let (first, second) = match left.cmp(&right) {
        Ordering::Greater => (&right, &left),
        _ => (&left, &right),
    };
    tagged_hash(hasher, TAPBRANCH_TAG_HASH, &[first, second])
}

pub fn transcript_hash<H: Sha256>(
    hasher: &mut H,
    previous: [u8; 32],
    sighash: [u8; 32],
    signature: [u8; 64],
) -> [u8; 32] {
    hasher.hash(&[&previous, &sighash, &signature])
}

fn tagged_hash<H: Sha256>(hasher: &mut H, tag_hash: [u8; 32], parts: &[&[u8]]) -> [u8; 32] {
    let mut all_parts: [&[u8]; 6] = [&[]; 6];
    all_parts[0] = &tag_hash;
    all_parts[1] = &tag_hash;
    for (target, source) in all_parts[2..].iter_mut().zip(parts.iter()) {
        *target = source;
    }
    hasher.hash(&all_parts[..2 + parts.len()])
}

fn p2tr_script(output_key: [u8; 32]) -> [u8; P2TR_SCRIPT_LEN] {
    let mut script = [0_u8; P2TR_SCRIPT_LEN];
    script[0] = 0x51;
    script[1] = 32;
    script[2..].copy_from_slice(&output_key);
    script
}

fn cooperative_vsize(input_count: u8, output_count: u8) -> u32 {
    let base_size = 4
        + 1
        + u32::from(input_count) * 41
        + 1
        + u32::from(output_count) * (8 + 1 + P2TR_SCRIPT_LEN as u32)
        + 4;
    let witness_per_input = 1
        + 2 * (1 + 64)
        + 1
        + COOPERATIVE_SCRIPT_LEN as u32
        + 1
        + COOPERATIVE_CONTROL_BLOCK_LEN as u32;
    let witness_size = 2 + u32::from(input_count) * witness_per_input;
    (base_size * 4 + witness_size).div_ceil(4)
}

fn serialize_outputs<const N: usize>(
    outputs: &[TxOutput],
    bytes: &mut ByteWriter<N>,
) -> Result<(), BenchmarkError> {
    for output in outputs {
        bytes.extend(&output.amount_sats.to_le_bytes())?;
        bytes.push(P2TR_SCRIPT_LEN as u8)?;
        bytes.extend(&output.script_pubkey)?;
    }
    Ok(())
}

struct ByteWriter<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> ByteWriter<N> {
    fn new() -> Self {
        Self {
            bytes: [0_u8; N],
            len: 0,
        }
    }

    fn push(&mut self, value: u8) -> Result<(), BenchmarkError> {
        self.extend(&[value])
    }

    fn extend(&mut self, values: &[u8]) -> Result<(), BenchmarkError> {
        let end = self
            .len
            .checked_add(values.len())
            .ok_or(BenchmarkError::SerializationOverflow)?;
        if end > N {
            return Err(BenchmarkError::SerializationOverflow);
        }
        self.bytes[self.len..end].copy_from_slice(values);
        self.len = end;
        Ok(())
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use bitcoin::{
        Amount, ScriptBuf, TxOut, XOnlyPublicKey,
        hashes::Hash,
        sighash::{Prevouts, SighashCache, TapSighashType},
        taproot::{LeafVersion, TapLeafHash, TapNodeHash, TapTweakHash},
    };
    use sha2::{Digest, Sha256 as Sha2};
    use std::vec::Vec;

    struct TestSha256;

    impl Sha256 for TestSha256 {
        fn hash(&mut self, parts: &[&[u8]]) -> [u8; 32] {
            let mut hasher = Sha2::new();
            for part in parts {
                hasher.update(part);
            }
            hasher.finalize().into()
        }
    }

    fn fixture(input_count: u8) -> BenchmarkConfig {
        let mut hasher = TestSha256;
        BenchmarkConfig::deterministic(&mut hasher, input_count, [7_u8; 32], [9_u8; 32]).unwrap()
    }

    #[test]
    fn policy_scripts_and_tag_hashes_match_the_protocol() {
        let phone = [1_u8; 32];
        let hww = [2_u8; 32];
        let cooperative = cooperative_script(phone, hww);
        assert_eq!(cooperative.len(), 70);
        assert_eq!(&cooperative[1..33], &phone);
        assert_eq!(&cooperative[35..67], &hww);
        assert_eq!(recovery_script(phone, PHONE_RECOVERY_BLOCKS).len(), 40);

        assert_eq!(TAPLEAF_TAG_HASH, <[u8; 32]>::from(Sha2::digest(b"TapLeaf")));
        assert_eq!(
            TAPBRANCH_TAG_HASH,
            <[u8; 32]>::from(Sha2::digest(b"TapBranch"))
        );
        assert_eq!(
            TAPTWEAK_TAG_HASH,
            <[u8; 32]>::from(Sha2::digest(b"TapTweak"))
        );
        assert_eq!(
            TAPSIGHASH_TAG_HASH,
            <[u8; 32]>::from(Sha2::digest(b"TapSighash"))
        );

        let mut hasher = TestSha256;
        let policy = PolicyCommitment::new(&mut hasher, phone, hww);
        let cooperative_leaf = TapLeafHash::from_script(
            ScriptBuf::from_bytes(cooperative.to_vec()).as_script(),
            LeafVersion::TapScript,
        );
        let phone_recovery_leaf = TapLeafHash::from_script(
            ScriptBuf::from_bytes(recovery_script(phone, PHONE_RECOVERY_BLOCKS).to_vec())
                .as_script(),
            LeafVersion::TapScript,
        );
        let hww_recovery_leaf = TapLeafHash::from_script(
            ScriptBuf::from_bytes(recovery_script(hww, HWW_RECOVERY_BLOCKS).to_vec()).as_script(),
            LeafVersion::TapScript,
        );
        let recoveries =
            TapNodeHash::from_node_hashes(phone_recovery_leaf.into(), hww_recovery_leaf.into());
        let merkle_root = TapNodeHash::from_node_hashes(cooperative_leaf.into(), recoveries);
        let nums = XOnlyPublicKey::from_slice(&BIP341_NUMS_XONLY).unwrap();
        let tweak = TapTweakHash::from_key_and_tweak(nums, Some(merkle_root));

        assert_eq!(
            policy.cooperative_leaf_hash,
            cooperative_leaf.to_byte_array()
        );
        assert_eq!(policy.merkle_root, merkle_root.to_byte_array());
        assert_eq!(policy.output_key_tweak, tweak.to_byte_array());
    }

    #[test]
    fn fixed_signing_digest_matches_its_named_preimage() {
        assert_eq!(
            FIXED_SIGNING_DIGEST,
            <[u8; 32]>::from(Sha2::digest(b"Anzen benchmark fixed BIP340 digest v1"))
        );
    }

    #[test]
    fn workload_scales_with_rollover_inputs() {
        for (inputs, expected_jobs) in [(1, 28), (2, 29), (12, 39)] {
            let config = fixture(inputs);
            let mut hasher = TestSha256;
            let mut jobs = 0;
            let summary = config
                .for_each_signature_job(&mut hasher, |_| {
                    jobs += 1;
                    Ok::<_, ()>(())
                })
                .unwrap();
            assert_eq!(summary.transactions, 28);
            assert_eq!(summary.signature_jobs, expected_jobs);
            assert_eq!(jobs, usize::from(expected_jobs));
        }
    }

    #[test]
    fn bip340_public_key_normalization_negates_odd_y() {
        let generator_even = hex_literal::hex!(
            "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            "483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8"
        );
        let generator_odd = hex_literal::hex!(
            "0479be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
            "b7c52588d95c3b9aa25b0403f1eef75702e84bb7597aabe663b82f6f04ef2777"
        );
        assert_eq!(
            normalize_bip340_public_key(generator_even).unwrap(),
            generator_even
        );
        assert_eq!(
            normalize_bip340_public_key(generator_odd).unwrap(),
            generator_even
        );
    }

    #[test]
    fn every_signature_message_matches_rust_bitcoin_bip341() {
        let config = fixture(12);
        let mut ours = Vec::new();
        let mut hasher = TestSha256;
        config
            .for_each_signature_job(&mut hasher, |job| {
                ours.push(job.sighash);
                Ok::<_, VisitError<()>>(())
            })
            .unwrap();

        let mut expected = Vec::new();
        let mut hasher = TestSha256;
        config
            .walk_transactions(&mut hasher, |_hasher, transaction| {
                let raw = transaction.serialize().unwrap();
                let decoded: bitcoin::Transaction =
                    bitcoin::consensus::deserialize(raw.as_slice()).unwrap();
                let prevouts = transaction
                    .inputs()
                    .iter()
                    .map(|input| TxOut {
                        value: Amount::from_sat(input.amount_sats),
                        script_pubkey: ScriptBuf::from_bytes(config.vault_script_pubkey.to_vec()),
                    })
                    .collect::<Vec<_>>();
                let leaf = TapLeafHash::from_byte_array(config.cooperative_leaf_hash);
                for index in 0..transaction.input_count as usize {
                    let sighash = SighashCache::new(&decoded)
                        .taproot_script_spend_signature_hash(
                            index,
                            &Prevouts::All(&prevouts),
                            leaf,
                            TapSighashType::Default,
                        )
                        .unwrap();
                    expected.push(sighash.to_byte_array());
                }
                Ok::<_, VisitError<()>>(())
            })
            .unwrap();
        assert_eq!(ours, expected);
    }
}
