//! A Ledger over Bluetooth Low Energy: APDU commands to whichever app is open on the device.

mod ble;
mod frame;
mod signer;
mod speculos;

pub use ble::{Ledger, LedgerFound, find, scan};
pub use signer::{HardwareSigner, Rejected};
pub use speculos::Speculos;

use std::time::Duration;

/// Anything that carries one APDU to a Ledger app and brings back its answer.
pub trait Exchange {
    fn exchange(
        &mut self,
        apdu: &[u8],
        timeout: Duration,
    ) -> impl Future<Output = anyhow::Result<Answer>> + Send;
}

/// What a Ledger answered: the data, then a two-byte status word (`0x9000` is success).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub data: Vec<u8>,
    pub status: u16,
}

impl Answer {
    pub const OK: u16 = 0x9000;

    pub(crate) fn parse(mut bytes: Vec<u8>) -> anyhow::Result<Self> {
        anyhow::ensure!(bytes.len() >= 2, "the Ledger's answer has no status word");
        let status = bytes.split_off(bytes.len() - 2);
        Ok(Self {
            data: bytes,
            status: u16::from_be_bytes([status[0], status[1]]),
        })
    }

    /// The data, if the device reported success.
    pub fn ok(self) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            self.status == Self::OK,
            "the Ledger answered with status {:#06x}",
            self.status
        );
        Ok(self.data)
    }
}

/// An APDU command: class, instruction, two parameters and up to 255 bytes of data.
pub fn apdu(cla: u8, ins: u8, p1: u8, p2: u8, data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let length =
        u8::try_from(data.len()).map_err(|_| anyhow::anyhow!("APDU data exceeds 255 bytes"))?;
    let mut command = vec![cla, ins, p1, p2, length];
    command.extend_from_slice(data);
    Ok(command)
}
