use trezor_app_sdk::{Error, Result};

pub const RUN_BENCHMARK: u16 = 1;
pub const BENCHMARK_RESULT: u16 = 2;
pub const PROTOCOL_VERSION: u8 = 1;
pub const ROLLOVER_INPUTS: u8 = 12;
pub const REQUEST_LEN: usize = 2;
pub const RESPONSE_LEN: usize = 84;

pub fn decode_request(data: &[u8]) -> Result<()> {
    if data.len() == REQUEST_LEN && data == [PROTOCOL_VERSION, ROLLOVER_INPUTS] {
        Ok(())
    } else {
        Err(Error::InvalidMessage)
    }
}

pub struct BenchmarkResponse {
    pub transactions: u8,
    pub signatures: u8,
    pub key_derivation_ms: u32,
    pub graph_ms: u32,
    pub signing_ms: u32,
    pub total_ms: u32,
    pub last_sighash: [u8; 32],
    pub last_signature_r: [u8; 32],
}

impl BenchmarkResponse {
    pub fn encode(&self) -> [u8; RESPONSE_LEN] {
        let mut response = [0_u8; RESPONSE_LEN];
        response[0] = PROTOCOL_VERSION;
        response[1] = ROLLOVER_INPUTS;
        response[2] = self.transactions;
        response[3] = self.signatures;
        response[4..8].copy_from_slice(&self.key_derivation_ms.to_le_bytes());
        response[8..12].copy_from_slice(&self.graph_ms.to_le_bytes());
        response[12..16].copy_from_slice(&self.signing_ms.to_le_bytes());
        response[16..20].copy_from_slice(&self.total_ms.to_le_bytes());
        response[20..52].copy_from_slice(&self.last_sighash);
        response[52..84].copy_from_slice(&self.last_signature_r);
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_is_versioned_and_fixed_to_the_real_rollover_shape() {
        assert!(decode_request(&[1, 12]).is_ok());
        assert!(decode_request(&[1, 1]).is_err());
        assert!(decode_request(b"run").is_err());
        assert_eq!(REQUEST_LEN, 2);
    }

    #[test]
    fn response_encoding_is_stable() {
        let response = BenchmarkResponse {
            transactions: 15,
            signatures: 26,
            key_derivation_ms: 34,
            graph_ms: 66,
            signing_ms: 668,
            total_ms: 768,
            last_sighash: [0xaa; 32],
            last_signature_r: [0xbb; 32],
        }
        .encode();
        assert_eq!(response.len(), RESPONSE_LEN);
        assert_eq!(&response[..4], &[1, 12, 15, 26]);
        assert_eq!(u32::from_le_bytes(response[4..8].try_into().unwrap()), 34);
        assert_eq!(
            u32::from_le_bytes(response[16..20].try_into().unwrap()),
            768
        );
        assert_eq!(&response[20..52], &[0xaa; 32]);
        assert_eq!(&response[52..84], &[0xbb; 32]);
    }
}
