//! What the Trezor shows: a review to approve, and progress while signing.

use crate::{Result, Stop};
use alloc::{format, string::String, vec::Vec};
use trezor_app_sdk::{
    self as sdk,
    ui::{self, ConfirmProperties, Property, TrezorUiResult},
};

/// Trezor's own `ButtonRequestType.SignTx`: tells the host what kind of screen is up.
const SIGN_TX: i32 = 8;

/// A review: named rows, then an approval (held, if `hold`). `Ok` only if the person approved.
/// `name` identifies the screen to the host while it waits.
pub fn review(
    title: &str,
    fields: &[(&'static str, String)],
    approve: &str,
    hold: bool,
    name: &str,
) -> Result<()> {
    let properties: Vec<Property> = fields
        .iter()
        .map(|(key, value)| {
            // Addresses and keys in the fixed-width face, sentences in the reading face.
            let mono = !value.contains(' ');
            Property::new(key, value, mono)
        })
        .collect();
    let answer = ui::confirm_properties(ConfirmProperties::new(
        title,
        &properties,
        None,
        Some(approve),
        hold,
        Some(name),
        SIGN_TX,
        false,
    ));
    match answer {
        Ok(TrezorUiResult::Confirmed) => Ok(()),
        Ok(_) | Err(sdk::Error::Cancelled) => Err(Stop::Rejected),
        Err(error) => Err(Stop::Core(error)),
    }
}

/// Progress through `total` signatures. The first call puts the screen up.
pub fn signing(signed: usize, total: usize) -> sdk::Result<()> {
    if signed == 0 {
        ui::init_progress(None, Some("Signing"), false, false)?;
    }
    let description = format!("{signed} of {total}");
    let value = if total == 0 {
        1000
    } else {
        signed * 1000 / total
    };
    ui::update_progress(Some(&description), value as u32)
}

pub fn signed() -> sdk::Result<()> {
    ui::end_progress()
}
