//! The vaults this device accepted, in the app's flash.
//!
//! Each record has two copies. A write goes to the older copy, with a higher sequence number and
//! a checksum, so a write cut short by power loss leaves the previous copy in force. A copy that
//! does not check out (blank flash included, which is how the emulator starts) is simply absent:
//! reading never fails, it finds a vault or it does not.

use crate::bolos::Bolos;
use alloc::{vec, vec::Vec};
use anzen_protocol::{address::Network, crypto::Crypto};
use ledger_device_sdk::{NVMData, sys};

/// One copy: a header, the record, a checksum. The largest signer state is under 1.9 KB.
const COPY: usize = 2_048;
const MAGIC: u8 = 0xa5;
/// Magic, sequence number, record length.
const HEADER: usize = 1 + 4 + 2;
const CHECKSUM: usize = 4;

/// Aligned and sized to whole flash pages, so no two copies share a page.
#[repr(C, align(512))]
struct Copies([[u8; COPY]; 2]);

macro_rules! record {
    ($name:ident) => {
        #[unsafe(link_section = ".nvm_data")]
        static mut $name: NVMData<Copies> = NVMData::new(Copies([[0; COPY]; 2]));
    };
}
record!(MAINNET);
record!(MUTINYNET);
record!(REGTEST);
record!(ROTATED);

/// Which record: the accepted vault of a network, or the vault an approved rotation moves to.
#[derive(Clone, Copy)]
pub enum Place {
    Vault(Network),
    Rotated,
}

fn copies(place: Place) -> &'static Copies {
    // The app is single-threaded. The reference is into flash: read through it, write only with
    // `nvm_write`.
    unsafe {
        match place {
            Place::Vault(Network::Mainnet) => (*&raw mut MAINNET).get_mut(),
            Place::Vault(Network::Mutinynet) => (*&raw mut MUTINYNET).get_mut(),
            Place::Vault(Network::Regtest) => (*&raw mut REGTEST).get_mut(),
            Place::Rotated => (*&raw mut ROTATED).get_mut(),
        }
    }
}

fn checksum(bytes: &[u8]) -> [u8; CHECKSUM] {
    let digest = Bolos.sha256(&[bytes]);
    [digest[0], digest[1], digest[2], digest[3]]
}

/// A copy's sequence number and record, if it is whole.
fn parse(copy: &[u8; COPY]) -> Option<(u32, &[u8])> {
    if copy[0] != MAGIC {
        return None;
    }
    let sequence = u32::from_be_bytes([copy[1], copy[2], copy[3], copy[4]]);
    let length = usize::from(u16::from_be_bytes([copy[5], copy[6]]));
    let end = HEADER + length;
    if end + CHECKSUM > COPY || copy[end..end + CHECKSUM] != checksum(&copy[..end]) {
        return None;
    }
    Some((sequence, &copy[HEADER..end]))
}

/// The newest whole copy: its index, sequence number and record.
fn newest(copies: &'static Copies) -> Option<(usize, u32, &'static [u8])> {
    copies
        .0
        .iter()
        .enumerate()
        .filter_map(|(index, copy)| parse(copy).map(|(sequence, record)| (index, sequence, record)))
        .max_by_key(|(_, sequence, _)| *sequence)
}

/// The stored record, if there is one. An empty record means there is none.
pub fn read(place: Place) -> Option<&'static [u8]> {
    newest(copies(place))
        .map(|(_, _, record)| record)
        .filter(|record| !record.is_empty())
}

/// Replace the record. An empty one clears it. Fails only if it is too large.
pub fn write(place: Place, record: &[u8]) -> Result<(), ()> {
    let end = HEADER + record.len();
    if end + CHECKSUM > COPY {
        return Err(());
    }
    let copies = copies(place);
    let (target, sequence) = match newest(copies) {
        Some((index, sequence, _)) => (1 - index, sequence.wrapping_add(1)),
        None => (0, 1),
    };
    let mut copy: Vec<u8> = vec![0; COPY];
    copy[0] = MAGIC;
    copy[1..5].copy_from_slice(&sequence.to_be_bytes());
    copy[5..7].copy_from_slice(&(record.len() as u16).to_be_bytes());
    copy[HEADER..end].copy_from_slice(record);
    let sum = checksum(&copy[..end]);
    copy[end..end + CHECKSUM].copy_from_slice(&sum);
    unsafe {
        sys::nvm_write(
            copies.0[target].as_ptr() as *mut core::ffi::c_void,
            copy.as_ptr() as *mut core::ffi::c_void,
            COPY as u32,
        );
    }
    Ok(())
}
