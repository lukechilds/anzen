//! A review as the rows a hardware signer's screen shows: a short name and a value each.
//!
//! The wording is the same on every device, so what a person learns to check on one holds for
//! another. `device` is what the device calls itself ("Ledger", "Trezor").

use crate::{OfflineActionReview, PolicyReview, RotationReview, SweepReview};
use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use anzen_protocol::{policy::Linkage, wire::OfflineAction};

pub type Fields = Vec<(&'static str, String)>;

/// Bitcoin amounts to at least four decimal places, and as many as the amount needs.
pub fn btc(sats: u64) -> String {
    let mut text = format!("{}.{:08}", sats / 100_000_000, sats % 100_000_000);
    while text.ends_with('0') && text.len() - text.find('.').expect("has a point") > 5 {
        text.pop();
    }
    text.push_str(" BTC");
    text
}

pub fn policy(review: &PolicyReview, device: &str) -> Fields {
    let monthly = if review.monthly_limit_sats > 0 && review.funded_withdrawals > 0 {
        format!(
            "{} a month, {} months funded",
            btc(review.monthly_limit_sats),
            review.funded_withdrawals
        )
    } else {
        "Off".to_string()
    };
    let emergency = match review.emergency_delay_seconds {
        Some(_) => format!("{} after 1 week", btc(review.emergency_access_limit_sats)),
        None => "Off".to_string(),
    };
    let previous = match review.previous_policy {
        Linkage::First => "None: this is the first".to_string(),
        Linkage::ConflictsWithPrevious | Linkage::ConsumesPrevious => {
            "Replaced by this one".to_string()
        }
        Linkage::MonthlyReportedComplete => format!(
            "Your phone reports its monthly allowances were all used. This {device} cannot check."
        ),
    };
    vec![
        ("Vault", review.vault_address.clone()),
        ("Vault balance", btc(review.vault_input_sats)),
        ("Monthly allowance", monthly),
        ("Emergency access", emergency),
        ("Previous policy", previous),
        (
            "Signs",
            format!(
                "{} transactions, {} sats fee",
                review.transaction_count, review.rollover_fee_sats
            ),
        ),
    ]
}

pub fn sweep(review: &SweepReview) -> Fields {
    vec![
        ("Move vault funds", btc(review.amount_sats)),
        ("To", review.destination.clone()),
        ("Fee", format!("{} sats", review.fee_sats)),
        ("From vault", review.vault_address.clone()),
    ]
}

/// The action's title, then its rows.
pub fn offline_action(review: &OfflineActionReview) -> (&'static str, Fields) {
    let (title, note) = match review.action {
        OfflineAction::HwwRecovery => (
            "Recover vault funds",
            "Bitcoin accepts this only about 388 days after each deposit confirmed. Signing does not unlock funds early.",
        ),
        OfflineAction::RevokeActions => (
            "Revoke vault actions",
            "Disables the phone's monthly and emergency withdrawals. Savings stay in the vault.",
        ),
    };
    let fields = vec![
        (title, btc(review.amount_sats)),
        ("To", review.destination.clone()),
        ("Fee", format!("{} sats", review.fee_sats)),
        ("From vault", review.vault_address.clone()),
        ("Note", note.to_string()),
    ];
    (title, fields)
}

pub fn rotation(review: &RotationReview) -> Fields {
    vec![
        ("Move to a new phone key", btc(review.amount_sats)),
        ("New phone key", review.new_phone_public_key.clone()),
        ("From vault", review.old_vault_address.clone()),
        ("To new vault", review.new_vault_address.clone()),
        (
            "Policy kept",
            format!(
                "{} a month ({} funded), {} emergency",
                btc(review.monthly_limit_sats),
                review.funded_withdrawals,
                btc(review.emergency_access_limit_sats)
            ),
        ),
        ("Fee", format!("{} sats", review.sweep_fee_sats)),
    ]
}

#[cfg(test)]
mod tests {
    use super::btc;

    #[test]
    fn amounts_show_four_places_and_no_more_than_they_need() {
        assert_eq!(btc(0), "0.0000 BTC");
        assert_eq!(btc(100_000_000), "1.0000 BTC");
        assert_eq!(btc(123_450_000), "1.2345 BTC");
        assert_eq!(btc(123_456_789), "1.23456789 BTC");
        assert_eq!(btc(1), "0.00000001 BTC");
        assert_eq!(btc(2_100_000_000_000_000), "21000000.0000 BTC");
    }
}
