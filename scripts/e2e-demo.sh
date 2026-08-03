#!/usr/bin/env bash
set -euo pipefail

DEMO_ROOT=/demo
MAIN="$DEMO_ROOT/main"
STOLEN_PHONE="$DEMO_ROOT/stolen-phone"
LOST_HWW="$DEMO_ROOT/lost-hww"
STOLEN_HWW="$DEMO_ROOT/stolen-hww"
NO_CLOUD="$DEMO_ROOT/lost-phone-no-cloud"
BOTH_LOST="$DEMO_ROOT/both-lost"
CLOUD_SOURCE="$DEMO_ROOT/cloud-source"
FORGOTTEN="$DEMO_ROOT/forgotten-rollover"
BOTH_COMPROMISED="$DEMO_ROOT/both-compromised"
PARTIAL="$DEMO_ROOT/partial-funding"

section() {
    printf '\n\n================================================================================\n'
    printf '%s\n' "$1"
    printf '================================================================================\n'
}

note() {
    printf '\n--- %s ---\n' "$1"
}

show_command() {
    printf '$ vault-cli --data-dir %q' "$1"
    shift
    printf ' %q' "$@"
    printf '\n'
}

vc() {
    local data_dir=$1
    shift
    show_command "$data_dir" "$@"
    vault-cli --data-dir "$data_dir" "$@"
}

capture_vc() {
    local variable_name=$1
    local data_dir=$2
    shift 2
    local output
    show_command "$data_dir" "$@"
    output=$(vault-cli --data-dir "$data_dir" "$@")
    printf '%s\n' "$output"
    printf -v "$variable_name" '%s' "$output"
}

expect_failure() {
    local explanation=$1
    local data_dir=$2
    shift 2
    local output
    local status
    printf '\nEXPECTED FAILURE: %s\n' "$explanation"
    show_command "$data_dir" "$@"
    set +e
    output=$(vault-cli --data-dir "$data_dir" "$@" 2>&1)
    status=$?
    set -e
    printf '%s\n' "$output"
    if [[ $status -eq 0 ]]; then
        printf 'ERROR: command unexpectedly succeeded\n' >&2
        exit 1
    fi
    printf 'Result: safely rejected (exit %s)\n' "$status"
}

node_height() {
    vault-cli --data-dir "$MAIN" node info | awk '/^Height:/ {print $2}'
}

node_mtp() {
    vault-cli --data-dir "$MAIN" node info | awk '/^Median time past:/ {print $4}'
}

advance_mtp_to() {
    local target=$1
    note "Advance Median Time Past strictly beyond $target ($(date -u -d "@$target" '+%Y-%m-%d %H:%M:%S UTC'))"
    vc "$MAIN" node set-time "$((target + 60))"
    while (( $(node_mtp) <= target )); do
        vc "$MAIN" node mine 1 "$MINING_ADDRESS"
    done
    vc "$MAIN" node info
}

mine_to_next_height() {
    local target=$1
    local label=$2
    local current
    local remaining
    local batch
    local mtp
    current=$(node_height)
    remaining=$((target - (current + 1)))
    if (( remaining < 0 )); then
        printf 'ERROR: target next height %s is behind current next height %s\n' "$target" "$((current + 1))" >&2
        exit 1
    fi
    note "$label: mine $remaining real blocks to next-block height $target"
    while (( remaining > 0 )); do
        batch=$remaining
        if (( batch > 5000 )); then
            batch=5000
        fi
        mtp=$(node_mtp)
        vc "$MAIN" node set-time "$((mtp + batch + 60))"
        vc "$MAIN" node mine "$batch" "$MINING_ADDRESS"
        remaining=$((remaining - batch))
    done
    if (( $(node_height) + 1 != target )); then
        printf 'ERROR: height advancement did not stop at requested next-block height\n' >&2
        exit 1
    fi
    vc "$MAIN" node info
}

init_scenario() {
    local data_dir=$1
    local label=$2
    local output
    output=$(vault-cli --data-dir "$data_dir" init --hard-limit-sats 100000)
    printf '%-28s %s\n' "$label" "$(printf '%s\n' "$output" | awk -F': ' '/^Vault address:/ {print $2}')"
    printf '  %s\n' "$(printf '%s\n' "$output" | awk -F': ' '/^Phone vault key:/ {print $0}')"
    printf '  %s\n' "$(printf '%s\n' "$output" | awk -F': ' '/^HWW vault key:/ {print $0}')"
}

fund_scenario() {
    local data_dir=$1
    local label=$2
    local vault_address
    vault_address=$(jq -r .vault_address "$data_dir/vault.json")
    note "Fund $label with 0.02 BTC"
    vc "$MAIN" node send "$vault_address" 2000000
}

section "RENEWABLE BITCOIN VAULT — COMPLETE REGTEST MVP DEMONSTRATION"
printf 'Demo started at: %s\n' "$(date -u '+%Y-%m-%d %H:%M:%S UTC')"
printf 'Bitcoin Core RPC: %s\n' "${VAULT_RPC_URL:-http://127.0.0.1:18443}"
printf 'All keys and funds below are disposable regtest data.\n'

if (( $(vault-cli --data-dir "$MAIN" node info | awk '/^Height:/ {print $2}') != 0 )); then
    printf 'ERROR: the narrative demo requires a fresh height-zero regtest node.\n' >&2
    printf 'Run ./scripts/run-e2e.sh so Compose resets the node first.\n' >&2
    exit 1
fi

section "1. SET UP THE SIMULATED PHONE, HWW, HOT WALLET, AND STATIC VAULT"
NOW=$(date -u +%s)
vc "$MAIN" init --hard-limit-sats 10000000
vc "$MAIN" policy

printf '\nHWW-encrypted phone backup metadata (ciphertext is local cloud stand-in):\n'
jq '{version, purpose, nonce_bytes: (.nonce | length), ciphertext_bytes: (.ciphertext | length)}' \
    "$MAIN/cloud/phone-seed-backup.json"

capture_vc HOT_ADDRESS_OUTPUT "$MAIN" hot-address
MINING_ADDRESS=$(printf '%s\n' "$HOT_ADDRESS_OUTPUT" | awk '/^Hot receive address:/ {print $4}')
vc "$MAIN" node set-time "$NOW"

note "Initialize independent funded states for every loss/theft scenario"
init_scenario "$STOLEN_PHONE" "Stolen phone"
init_scenario "$LOST_HWW" "Lost HWW"
init_scenario "$STOLEN_HWW" "Stolen HWW"
init_scenario "$NO_CLOUD" "Lost phone, no cloud"
init_scenario "$BOTH_LOST" "Both devices lost"
init_scenario "$CLOUD_SOURCE" "Cloud compromised"
init_scenario "$FORGOTTEN" "Rollover forgotten"
init_scenario "$BOTH_COMPROMISED" "Both keys compromised"
init_scenario "$PARTIAL" "Only three months fundable"

section "2. MINE SPENDABLE REGTEST COINS AND FUND THE MAIN VAULT WITH EXACTLY 2 BTC"
vc "$MAIN" node mine 101 "$MINING_ADDRESS"
MAIN_VAULT_ADDRESS=$(jq -r .vault_address "$MAIN/vault.json")
vc "$MAIN" node send "$MAIN_VAULT_ADDRESS" 200000000
fund_scenario "$STOLEN_PHONE" "stolen-phone scenario"
fund_scenario "$LOST_HWW" "lost-HWW scenario"
fund_scenario "$STOLEN_HWW" "stolen-HWW scenario"
fund_scenario "$NO_CLOUD" "lost-phone/no-cloud scenario"
fund_scenario "$BOTH_LOST" "both-lost scenario"
fund_scenario "$CLOUD_SOURCE" "cloud-compromise scenario"
fund_scenario "$FORGOTTEN" "forgotten-rollover scenario"
fund_scenario "$BOTH_COMPROMISED" "both-compromised scenario"
PARTIAL_ADDRESS=$(jq -r .vault_address "$PARTIAL/vault.json")
note "Fund partial-rollover scenario with only 0.0035 BTC"
vc "$MAIN" node send "$PARTIAL_ADDRESS" 350000
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
FUNDING_HEIGHT=$(node_height)
printf 'All scenario vault outputs confirmed at height: %s\n' "$FUNDING_HEIGHT"
vc "$MAIN" status

note "Insufficient balance does not fail rollover: warn and fund the earliest months only"
vc "$PARTIAL" ceremony prepare --now "$NOW"
vc "$PARTIAL" ceremony approve --yes
vc "$PARTIAL" ceremony finalize
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$PARTIAL" status

section "3. ANNUAL SIGNING CEREMONY: 12 EQUAL CHUNKS, AUTHORIZATIONS, AND REVOCATIONS"
vc "$MAIN" ceremony prepare --now "$NOW"
vc "$MAIN" ceremony approve --yes
vc "$MAIN" ceremony finalize
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$MAIN" status

printf '\nIndividually encrypted phone transaction artifacts:\n'
for artifact in "$MAIN"/phone/transactions/*.json; do
    jq -r '"\(.month) \(.kind): txid=\(.txid) nonce-bytes=\(.encrypted_transaction.nonce|length) ciphertext-bytes=\(.encrypted_transaction.ciphertext|length)"' "$artifact"
done

FIRST_MONTH=$(jq -r '.entries[0].month' "$MAIN/phone/schedule.json")
SECOND_MONTH=$(jq -r '.entries[1].month' "$MAIN/phone/schedule.json")
FIRST_UNLOCK=$(jq -r '.entries[0].unlock_timestamp' "$MAIN/phone/schedule.json")
SECOND_UNLOCK=$(jq -r '.entries[1].unlock_timestamp' "$MAIN/phone/schedule.json")
printf '\nCurrent-date-derived schedule: first=%s (%s), second=%s (%s)\n' \
    "$FIRST_MONTH" "$(date -u -d "@$FIRST_UNLOCK" '+%Y-%m-%d %H:%M:%S UTC')" \
    "$SECOND_MONTH" "$(date -u -d "@$SECOND_UNLOCK" '+%Y-%m-%d %H:%M:%S UTC')"

section "4. CALENDAR ALLOWANCES, DYNAMIC SOFT LIMIT, AND PHONE-ONLY REVOCATION"
expect_failure "the first authorization is non-final before 00:00 UTC on the first of its month" \
    "$MAIN" monthly "$FIRST_MONTH" authorize

advance_mtp_to "$FIRST_UNLOCK"
vc "$MAIN" monthly "$FIRST_MONTH" authorize
vc "$MAIN" soft-limit "$FIRST_MONTH" 1000000
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
printf 'The hard authorization was 0.1 BTC; the phone retained at most 0.01 BTC and returned exactly 0.09 BTC to cold storage, with its fee paid from hot funds.\n'
vc "$MAIN" status

note "Extracted phone key steals ordinary hot-wallet funds immediately"
capture_vc THIEF_ADDRESS_OUTPUT "$BOTH_COMPROMISED" hot-address
THIEF_ADDRESS=$(printf '%s\n' "$THIEF_ADDRESS_OUTPUT" | awk '/^Hot receive address:/ {print $4}')
EXTRACTED_PHONE="$DEMO_ROOT/extracted-main-phone"
cp -a "$MAIN" "$EXTRACTED_PHONE"
rm -- "$EXTRACTED_PHONE/hww/device.json"
vc "$EXTRACTED_PHONE" node send "$THIEF_ADDRESS" 500000
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
printf 'The copied phone could spend hot funds, but its cold-vault sweep still requires H or CSV maturity.\n'
expect_failure "extracted phone key cannot immediately spend the cold vault" \
    "$EXTRACTED_PHONE" sweep cooperative "$THIEF_ADDRESS"

advance_mtp_to "$((FIRST_UNLOCK + 14 * 24 * 60 * 60))"
vc "$MAIN" monthly "$SECOND_MONTH" revoke
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
printf 'The %s revocation returned its entire chunk (less 1 sat/vB fee) to the static vault.\n' "$SECOND_MONTH"

advance_mtp_to "$SECOND_UNLOCK"
expect_failure "the revoked monthly authorization conflicts with an already-confirmed revocation" \
    "$MAIN" monthly "$SECOND_MONTH" authorize

section "5. LOST PHONE: HWW-DECRYPTED BACKUP, NEW PHONE KEY, NEW STATIC ADDRESS"
rm -- "$MAIN/phone/device.json"
expect_failure "the hot wallet cannot open after the phone key is deleted" "$MAIN" hot-address
vc "$MAIN" restore-phone
OLD_MAIN_ADDRESS=$(jq -r .vault_address "$MAIN/vault.json")
vc "$MAIN" rotate-phone
NEW_MAIN_ADDRESS=$(jq -r .vault_address "$MAIN/vault.json")
if [[ $OLD_MAIN_ADDRESS == "$NEW_MAIN_ADDRESS" ]]; then
    printf 'ERROR: emergency key rotation reused the vault address\n' >&2
    exit 1
fi
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$MAIN" status
printf '\nArchived obsolete public state and presigned ciphertext:\n'
find "$MAIN/history" -maxdepth 4 -type f -print | sort

note "Recreate the annual schedule under the recovered phone's new key epoch"
CURRENT_MTP=$(node_mtp)
vc "$MAIN" ceremony prepare --now "$CURRENT_MTP"
vc "$MAIN" ceremony approve --yes
vc "$MAIN" ceremony finalize
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$MAIN" status

capture_vc RECOVERY_ADDRESS_OUTPUT "$MAIN" hot-address
RECOVERY_ADDRESS=$(printf '%s\n' "$RECOVERY_ADDRESS_OUTPUT" | awk '/^Hot receive address:/ {print $4}')
capture_vc ATTACKER_ADDRESS_OUTPUT "$MAIN" hot-address
ATTACKER_ADDRESS=$(printf '%s\n' "$ATTACKER_ADDRESS_OUTPUT" | awk '/^Hot receive address:/ {print $4}')

section "6. IMMEDIATE LOSS/THEFT SCENARIOS BEFORE RECOVERY PATHS MATURE"

note "Cloud account compromised: ciphertext without the HWW cannot restore the phone"
CLOUD_ATTACKER="$DEMO_ROOT/cloud-attacker"
mkdir -p "$CLOUD_ATTACKER/cloud"
cp "$CLOUD_SOURCE/vault.json" "$CLOUD_ATTACKER/vault.json"
cp "$CLOUD_SOURCE/cloud/phone-seed-backup.json" "$CLOUD_ATTACKER/cloud/phone-seed-backup.json"
expect_failure "the cloud attacker has no HWW decryption key" "$CLOUD_ATTACKER" restore-phone

note "Both keys compromised: the immediate cooperative path can steal the full vault"
vc "$BOTH_COMPROMISED" sweep cooperative "$ATTACKER_ADDRESS"
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$BOTH_COMPROMISED" status

note "Stolen phone: copied M cannot spend cold funds yet; HWW owner restores M and rotates immediately"
STOLEN_PHONE_ATTACKER="$DEMO_ROOT/stolen-phone-attacker"
cp -a "$STOLEN_PHONE" "$STOLEN_PHONE_ATTACKER"
rm -- "$STOLEN_PHONE_ATTACKER/hww/device.json"
expect_failure "stolen phone lacks H for the immediate 2-of-2 path" \
    "$STOLEN_PHONE_ATTACKER" sweep cooperative "$ATTACKER_ADDRESS"
expect_failure "stolen phone's 61,200-block fallback is not mature" \
    "$STOLEN_PHONE_ATTACKER" sweep phone-recovery "$ATTACKER_ADDRESS"
rm -- "$STOLEN_PHONE/phone/device.json"
vc "$STOLEN_PHONE" restore-phone
vc "$STOLEN_PHONE" rotate-phone
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$STOLEN_PHONE" status

note "Lost HWW: presign first, remove H, then continue using a phone-held monthly authorization"
CURRENT_MTP=$(node_mtp)
vc "$LOST_HWW" ceremony prepare --now "$CURRENT_MTP"
vc "$LOST_HWW" ceremony approve --yes
vc "$LOST_HWW" ceremony finalize
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
rm -- "$LOST_HWW/hww/device.json"
vc "$LOST_HWW" hot-address
LOST_HWW_MONTH=$(jq -r '.entries[0].month' "$LOST_HWW/phone/schedule.json")
LOST_HWW_UNLOCK=$(jq -r '.entries[0].unlock_timestamp' "$LOST_HWW/phone/schedule.json")
advance_mtp_to "$LOST_HWW_UNLOCK"
vc "$LOST_HWW" monthly "$LOST_HWW_MONTH" authorize
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
LOST_HWW_LATEST_HEIGHT=$(node_height)
vc "$LOST_HWW" status

note "Stolen HWW: attacker gets H only; legitimate state keeps M only"
STOLEN_HWW_ATTACKER="$DEMO_ROOT/stolen-hww-attacker"
cp -a "$STOLEN_HWW" "$STOLEN_HWW_ATTACKER"
rm -- "$STOLEN_HWW_ATTACKER/phone/device.json"
rm -- "$STOLEN_HWW/hww/device.json"
expect_failure "stolen HWW cannot use its later fallback during the phone priority window" \
    "$STOLEN_HWW_ATTACKER" sweep hww-recovery "$ATTACKER_ADDRESS"

note "Lost phone and unavailable cloud backup: only the eventual HWW fallback remains"
rm -- "$NO_CLOUD/phone/device.json" "$NO_CLOUD/cloud/phone-seed-backup.json"
expect_failure "there is no encrypted phone backup to restore" "$NO_CLOUD" restore-phone
expect_failure "the HWW fallback is not mature yet" \
    "$NO_CLOUD" sweep hww-recovery "$RECOVERY_ADDRESS"

note "Both devices lost: encrypted backup remains, but its HWW decryption key is gone"
rm -- "$BOTH_LOST/phone/device.json" "$BOTH_LOST/hww/device.json"
expect_failure "phone backup cannot be decrypted without either surviving device" "$BOTH_LOST" restore-phone

note "Rollover forgotten: status reports activation heights from the oldest live UTXO"
vc "$FORGOTTEN" status

section "7. MINE THE REAL 61,200-BLOCK PHONE RECOVERY DELAY"
PHONE_TARGET=$((LOST_HWW_LATEST_HEIGHT + 61200))
mine_to_next_height "$PHONE_TARGET" "Phone-only recovery activation"

note "Lost HWW: phone recovers every now-mature vault UTXO alone"
vc "$LOST_HWW" sweep phone-recovery "$RECOVERY_ADDRESS"

note "Stolen HWW: legitimate phone uses its approximately one-month priority window"
vc "$STOLEN_HWW" sweep phone-recovery "$RECOVERY_ADDRESS"
expect_failure "HWW attacker is still below its 65,535-block fallback" \
    "$STOLEN_HWW_ATTACKER" sweep hww-recovery "$ATTACKER_ADDRESS"

note "The old stolen-phone key sees no UTXO after the owner's emergency rotation"
expect_failure "rotation already invalidated the stolen phone's old policy outputs" \
    "$STOLEN_PHONE_ATTACKER" sweep phone-recovery "$ATTACKER_ADDRESS"

note "Forgotten rollover now shows phone recovery active while HWW recovery remains pending"
vc "$FORGOTTEN" status
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$LOST_HWW" status
vc "$STOLEN_HWW" status

section "8. MINE TO THE REAL 65,535-BLOCK HWW RECOVERY DELAY"
HWW_TARGET=$((FUNDING_HEIGHT + 65535))
mine_to_next_height "$HWW_TARGET" "HWW-only recovery activation"

note "Lost phone plus no cloud: surviving HWW now recovers the vault alone"
vc "$NO_CLOUD" sweep hww-recovery "$RECOVERY_ADDRESS"

note "Stolen HWW attacker is too late because the phone swept during its priority window"
expect_failure "the old vault output was spent by the legitimate phone" \
    "$STOLEN_HWW_ATTACKER" sweep hww-recovery "$ATTACKER_ADDRESS"

note "Cloud ciphertext still grants no signing or decryption capability at maturity"
expect_failure "cloud-only attacker has no HWW key" \
    "$CLOUD_ATTACKER" sweep hww-recovery "$ATTACKER_ADDRESS"

note "Both devices permanently lost: consensus paths are mature but no signing key survives"
expect_failure "no HWW key exists to sign the mature HWW path" \
    "$BOTH_LOST" sweep hww-recovery "$RECOVERY_ADDRESS"
printf 'Without the optional social-recovery mechanism (out of MVP scope), these funds are unrecoverable.\n'

note "Forgotten rollover now shows both single-device paths active"
vc "$FORGOTTEN" status
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$NO_CLOUD" status

note "A late cooperative rollover still renews the forgotten vault and resets both timers"
CURRENT_MTP=$(node_mtp)
vc "$FORGOTTEN" ceremony prepare --now "$CURRENT_MTP"
vc "$FORGOTTEN" ceremony approve --yes
vc "$FORGOTTEN" ceremony finalize
vc "$MAIN" node mine 1 "$MINING_ADDRESS"
vc "$FORGOTTEN" status

section "9. DEMO COMPLETE"
printf 'Verified on a real Bitcoin Core regtest node:\n'
printf '  - exact 2 BTC funding and 12 equal cold chunks\n'
printf '  - one HWW policy approval for all authorization/revocation PSBTs\n'
printf '  - first-of-month UTC/MTP enforcement, 0.1 BTC hard limit, 0.01 BTC soft limit\n'
printf '  - phone-only pre-maturity revocation and rejected conflicting authorization\n'
printf '  - HWW-encrypted phone backup restoration and emergency static-address rotation\n'
printf '  - all documented device-loss, key-theft, cloud, and forgotten-rollover outcomes\n'
printf '  - real 61,200-block and 65,535-block CSV recovery spends\n'
