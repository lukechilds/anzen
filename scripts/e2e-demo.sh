#!/usr/bin/env bash
set -euo pipefail

readonly DEMO_ROOT=/demo
readonly DEFAULT_MONTHLY_LIMIT_SATS=10000000
readonly DEFAULT_FUNDING_SATS=200000000
readonly PHONE_RECOVERY_BLOCKS=61200
readonly HWW_RECOVERY_BLOCKS=65535
readonly BLOCKS_PER_YEAR=52560

CLI_OUTPUT_COLOR=$'\033[90m'
COLOR_RESET=$'\033[0m'
if [[ -n ${NO_COLOR:-} ]]; then
    CLI_OUTPUT_COLOR=
    COLOR_RESET=
fi
readonly CLI_OUTPUT_COLOR COLOR_RESET

readonly TESTS=(
    setup-policy
    monthly-spend
    monthly-revoke
    partial-funding
    lost-phone
    stolen-phone
    lost-hww
    stolen-hww
    lost-phone-no-cloud
    both-lost
    cloud-compromise
    social-recovery
    both-compromised
    rollover-on-time
    rollover-forgotten
)

list_tests() {
    printf '%s\n' "${TESTS[@]}"
}

usage() {
    printf 'Usage: %s <test>\n\n' "$0"
    printf 'Run exactly one isolated end-to-end test. Use scripts/run-e2e.sh to run\n'
    printf 'one, several, or all tests with a fresh regtest node for each test.\n\n'
    printf 'Available tests:\n'
    list_tests | sed 's/^/  /'
}

if [[ ${1:-} == --list ]]; then
    list_tests
    exit 0
fi

if [[ $# -ne 1 ]]; then
    usage >&2
    exit 2
fi

E2E_TEST=$1
case " ${TESTS[*]} " in
    *" $E2E_TEST "*) ;;
    *)
        printf 'Unknown test: %s\n\n' "$E2E_TEST" >&2
        usage >&2
        exit 2
        ;;
esac

MAIN="$DEMO_ROOT/$E2E_TEST"
MINING_ADDRESS=
NOW=
FUNDING_HEIGHT=

step() {
    printf '\n--- %s ---\n' "$1"
}

success() {
    printf '\n✅ %s\n' "$1"
}

print_cli_output() {
    local output=$1
    if [[ -n $output ]]; then
        printf '%s%s%s\n' "$CLI_OUTPUT_COLOR" "$output" "$COLOR_RESET"
    fi
}

show_command() {
    shift
    printf '$ vault'
    printf ' %q' "$@"
    printf '\n'
}

show_file_command() {
    printf '$'
    printf ' %q' "$@"
    printf '\n'
}

vault() {
    local data_dir=$1
    local output
    local vault_exit
    shift
    show_command "$data_dir" "$@"
    set +e
    output=$(command vault --data-dir "$data_dir" "$@" 2>&1)
    vault_exit=$?
    set -e
    print_cli_output "$output"
    return "$vault_exit"
}

vault_capture() {
    local variable_name=$1
    local data_dir=$2
    local output
    shift 2
    show_command "$data_dir" "$@"
    if ! output=$(command vault --data-dir "$data_dir" "$@" 2>&1); then
        print_cli_output "$output"
        return 1
    fi
    print_cli_output "$output"
    printf -v "$variable_name" '%s' "$output"
}

vault_silent() {
    local data_dir=$1
    local output
    local status
    shift
    set +e
    output=$(command vault --data-dir "$data_dir" "$@" 2>&1)
    status=$?
    set -e
    if [[ $status -ne 0 ]]; then
        print_cli_output "$output" >&2
        return "$status"
    fi
}

vault_filtered() {
    local filter=$1
    local data_dir=$2
    local output
    shift 2
    show_command "$data_dir" "$@"
    if ! output=$(command vault --data-dir "$data_dir" "$@" 2>&1); then
        print_cli_output "$output" >&2
        return 1
    fi
    output=$(printf '%s\n' "$output" | awk "$filter")
    print_cli_output "$output"
}

expect_failure() {
    local explanation=$1
    local data_dir=$2
    local output
    local status
    local error_line
    shift 2
    printf '\nExpected rejection: %s\n' "$explanation"
    show_command "$data_dir" "$@"
    set +e
    output=$(command vault --data-dir "$data_dir" "$@" 2>&1)
    status=$?
    set -e
    if [[ $status -eq 0 ]]; then
        printf 'ERROR: command unexpectedly succeeded\n' >&2
        exit 1
    fi
    error_line=$(printf '%s\n' "$output" | sed -n '/^Error:/ {p;q;}')
    if [[ -n $error_line ]]; then
        print_cli_output "$error_line"
    else
        error_line=$(printf '%s\n' "$output" | sed -n '1p')
        print_cli_output "$error_line"
    fi
    printf 'Safely rejected.\n'
}

node_height() {
    command vault --data-dir "$MAIN" node info | awk '/^Height:/ {print $2}'
}

node_mtp() {
    command vault --data-dir "$MAIN" node info | awk '/^Median time past:/ {print $4}'
}

format_duration() {
    local seconds=$1
    if (( seconds < 60 )); then
        printf '%ss' "$seconds"
    elif (( seconds < 3600 )); then
        printf '%sm%02ss' "$((seconds / 60))" "$((seconds % 60))"
    else
        printf '%sh%02sm' "$((seconds / 3600))" "$(((seconds % 3600) / 60))"
    fi
}

mining_progress() {
    local completed=$1
    local total=$2
    local started_at=$3
    local now elapsed percent filled empty eta
    now=$(date +%s)
    elapsed=$((now - started_at))
    percent=$((completed * 100 / total))
    filled=$((percent / 5))
    empty=$((20 - filled))
    eta=0
    if (( completed > 0 && completed < total )); then
        eta=$((elapsed * (total - completed) / completed))
    fi
    printf '\rMining: ['
    printf '%*s' "$filled" '' | tr ' ' '#'
    printf '%*s' "$empty" '' | tr ' ' '-'
    printf '] %3s%% (%s/%s blocks), ETA %s' \
        "$percent" "$completed" "$total" "$(format_duration "$eta")"
}

mine_blocks() {
    local blocks=$1
    local label=$2
    local completed=0
    local batch
    local mtp
    local started_at
    show_command "$MAIN" node mine "$blocks" "$MINING_ADDRESS"
    if (( blocks <= 5000 )); then
        vault_silent "$MAIN" node mine "$blocks" "$MINING_ADDRESS"
        printf '%s: mined %s block(s); height is now %s.\n' \
            "$label" "$blocks" "$(node_height)"
        return
    fi
    printf '%s; this may take a while.\n' "$label"
    started_at=$(date +%s)
    while (( completed < blocks )); do
        batch=$((blocks - completed))
        if (( batch > 5000 )); then
            batch=5000
        fi
        if (( blocks > 5000 )); then
            mtp=$(node_mtp)
            vault_silent "$MAIN" node set-time "$((mtp + batch + 60))"
        fi
        vault_silent "$MAIN" node mine "$batch" "$MINING_ADDRESS"
        completed=$((completed + batch))
        mining_progress "$completed" "$blocks" "$started_at"
    done
    printf '\nReached block height %s.\n' "$(node_height)"
}

mine_to_next_height() {
    local target=$1
    local label=$2
    local current
    local remaining
    current=$(node_height)
    remaining=$((target - (current + 1)))
    if (( remaining < 0 )); then
        printf 'ERROR: next-block target %s is behind current next-block height %s\n' \
            "$target" "$((current + 1))" >&2
        exit 1
    fi
    if (( remaining == 0 )); then
        printf 'Recovery path is already valid at next-block height %s.\n' "$target"
        return
    fi
    mine_blocks "$remaining" "$label"
    if (( $(node_height) + 1 != target )); then
        printf 'ERROR: height advancement did not stop at next-block height %s\n' "$target" >&2
        exit 1
    fi
}

advance_calendar_to() {
    local target=$1
    local label=$2
    local target_display
    target_display=$(date -u -d "@$target" '+%Y-%m-%d %H:%M:%S UTC')
    step "$label"
    show_command "$MAIN" node set-time "$((target + 60))"
    vault_silent "$MAIN" node set-time "$((target + 60))"
    show_command "$MAIN" node mine 11 "$MINING_ADDRESS"
    vault_silent "$MAIN" node mine 11 "$MINING_ADDRESS"
    if (( $(node_mtp) <= target )); then
        printf 'ERROR: Median Time Past did not advance beyond %s\n' "$target_display" >&2
        exit 1
    fi
    printf 'Regtest calendar is now beyond %s.\n' "$target_display"
    success "$label complete."
}

init_vault() {
    vault_filtered '
        /^(Simulated phone initialized|Phone mnemonic:|Phone vault key:)/ { print }
    ' "$MAIN" phone init
    vault_filtered '
        /^(Simulated HWW initialized|HWW mnemonic:|HWW vault key:|HWW ready)/ { print }
    ' "$MAIN" hww init
    vault_filtered '
        /^(Vault initialized|Cold storage descriptor:|Vault address:|Phone recovery:|HWW recovery:|Monthly spending:|Cloud recovery backup:)/ { print }
    ' "$MAIN" init
}

show_backup_metadata() {
    printf 'Encrypted cloud payload: random symmetric key, %s-byte nonce, %s-byte ciphertext; key wrapped for HWW and %s friend(s).\n' \
        "$(jq -r '.encrypted_payload.nonce | length' "$MAIN/cloud/phone-seed-backup.json")" \
        "$(jq -r '.encrypted_payload.ciphertext | length' "$MAIN/cloud/phone-seed-backup.json")" \
        "$(jq -r '.friends | length' "$MAIN/cloud/phone-seed-backup.json")"
}

ceremony() {
    local now=$1
    local monthly_limit=${2:-$DEFAULT_MONTHLY_LIMIT_SATS}
    local proposal="${E2E_TEST}-policy.json"
    local approved="${E2E_TEST}-approved-policy.json"
    vault_filtered '
        /^(PHONE POLICY PROPOSAL|Cold storage descriptor:|Vault address:|Monthly spending:|Monthly limit:|Fee rate:|Total input:|Monthly pairs:|WARNING:|Rollover txid:|Rollover fee:|Deferred split txid:|Deferred split fee:|Exact monthly UTXO:|Split remainder:|Phone signed PSBTs:|Phone-signed policy proposal:)/ { print }
    ' "$MAIN" phone set-policy --monthly-limit "$monthly_limit" \
        --output "$proposal" --now "$now"
    vault_filtered '
        /^(SIMULATED HWW|Monthly spending:|Monthly limit:|Monthly pairs:|Rollover txid:|Deferred split txid:|Exact monthly UTXO:|Split remainder:|Phone signed PSBTs:|HWW validated and signed|HWW-approved policy:)/ { print }
    ' "$MAIN" hww confirm-policy "$proposal" --output "$approved" --yes
    vault_filtered '
        /^(Rollover broadcast:|Deferred monthly split encrypted:|Active monthly limit:|Encrypted monthly transaction pairs:)/ { print }
    ' "$MAIN" phone activate-policy "$approved"
}

status_compact() {
    vault_filtered '
        /^(Network:|Height:|Vault UTXOs:|Vault balance:)/ { print }
    ' "$MAIN" status
}

setup_vault() {
    local funding_sats=${1:-$DEFAULT_FUNDING_SATS}
    local hot_output
    local vault_address

    NOW=$(date -u +%s)
    step "Set up the simulated phone, HWW, hot wallet, and static vault"
    init_vault
    show_backup_metadata
    vault_capture hot_output "$MAIN" phone receive-address
    MINING_ADDRESS=$(printf '%s\n' "$hot_output" | awk '/^Hot receive address:/ {print $4}')
    vault "$MAIN" node set-time "$NOW"
    success "Phone, HWW, hot wallet, and vault configured."

    step "Mine spendable regtest coins and fund the vault"
    mine_blocks 101 "Creating spendable regtest coins"
    vault_address=$(jq -r .vault_address "$MAIN/vault.json")
    vault "$MAIN" phone send "$vault_address" "$funding_sats"
    mine_blocks 1 "Confirming the vault funding transaction"
    FUNDING_HEIGHT=$(node_height)
    status_compact
    success "Vault funding confirmed."
}

make_receiver() {
    local variable_name=$1
    local label=$2
    local receiver_dir="$DEMO_ROOT/receiver-$label"
    local receiver_output
    step "$label creates a fresh receiving wallet"
    show_command "$receiver_dir" phone init
    command vault --data-dir "$receiver_dir" phone init >/dev/null
    command vault --data-dir "$receiver_dir" hww init >/dev/null
    command vault --data-dir "$receiver_dir" init >/dev/null
    vault_capture receiver_output "$receiver_dir" phone receive-address
    printf -v "$variable_name" '%s' "$(printf '%s\n' "$receiver_output" | awk '/^Hot receive address:/ {print $4}')"
    success "$label receiving address ready."
}

confirm_transaction() {
    mine_blocks 1 "$1"
}

restore_phone() {
    local data_dir=$1
    local recovery="${E2E_TEST}-phone-recovery.json"
    vault "$data_dir" hww decrypt-phone-backup \
        "$data_dir/cloud/phone-seed-backup.json" --output "$recovery"
    vault "$data_dir" phone restore "$recovery"
}

rotate_phone_key() {
    local data_dir=$1
    local proposal="${E2E_TEST}-rotation.json"
    local approved="${E2E_TEST}-approved-rotation.json"
    vault "$data_dir" phone rotate-key --output "$proposal"
    vault "$data_dir" hww confirm-rotation "$proposal" --output "$approved" --yes
    vault "$data_dir" phone activate-rotation "$approved"
}

cooperative_sweep() {
    local data_dir=$1
    local destination=$2
    local proposal="${E2E_TEST}-sweep.json"
    local approved="${E2E_TEST}-approved-sweep.json"
    vault "$data_dir" phone create-sweep "$destination" --output "$proposal"
    vault "$data_dir" hww confirm-sweep "$proposal" --output "$approved" --yes
    vault "$data_dir" phone broadcast-sweep "$approved"
}

test_setup_policy() {
    NOW=$(date -u +%s)
    step "Set up the simulated phone, HWW, hot wallet, and static vault"
    init_vault
    show_backup_metadata
    vault "$MAIN" policy
    printf 'Policy timestamps are derived from the current UTC date when a ceremony begins.\n'
    success "Vault policy configured and verified."
}

test_monthly_spend() {
    local first_month first_unlock
    setup_vault

    step "Approve one annual policy and presign all monthly transactions"
    ceremony "$NOW"
    confirm_transaction "Confirming the annual rollover"
    first_month=$(jq -r '.entries[0].month' "$MAIN/phone/schedule.json")
    first_unlock=$(jq -r '.entries[0].unlock_timestamp' "$MAIN/phone/schedule.json")
    printf 'First allowance: %s at %s.\n' \
        "$first_month" "$(date -u -d "@$first_unlock" '+%Y-%m-%d %H:%M:%S UTC')"
    printf 'Each authorization and revocation is stored as its own phone-key-encrypted artifact.\n'
    success "Twelve monthly transaction pairs presigned."

    step "Attempt the first allowance before it unlocks"
    expect_failure "the allowance is locked before 00:00 UTC on the first of its month" \
        "$MAIN" phone authorize "$first_month"
    success "Pre-unlock allowance correctly rejected."

    advance_calendar_to "$first_unlock" "Fast-forward to the first monthly allowance"

    step "Execute the allowance with a lower soft limit"
    vault "$MAIN" phone authorize "$first_month"
    vault "$MAIN" phone apply-soft-limit "$first_month" --limit 1000000
    confirm_transaction "Confirming the authorization and soft-limit return"
    printf 'The 0.1 BTC monthly allowance retained 0.01 BTC hot and returned 0.09 BTC to cold storage (fees paid from hot funds).\n'
    status_compact
    success "0.01 BTC retained; 0.09 BTC returned cold."
}

test_monthly_revoke() {
    local second_month second_unlock first_unlock revoke_time
    setup_vault

    step "Approve one annual policy and presign all monthly transactions"
    ceremony "$NOW"
    confirm_transaction "Confirming the annual rollover"
    first_unlock=$(jq -r '.entries[0].unlock_timestamp' "$MAIN/phone/schedule.json")
    second_month=$(jq -r '.entries[1].month' "$MAIN/phone/schedule.json")
    second_unlock=$(jq -r '.entries[1].unlock_timestamp' "$MAIN/phone/schedule.json")
    revoke_time=$((first_unlock + 14 * 24 * 60 * 60))
    success "Twelve monthly transaction pairs presigned."

    advance_calendar_to "$revoke_time" "Fast-forward two weeks into the first allowance month"
    step "Revoke the next month's allowance from the phone before it unlocks"
    vault "$MAIN" phone revoke "$second_month"
    confirm_transaction "Confirming the phone-only revocation"
    printf 'The %s chunk returned to the static vault before its allowance became spendable.\n' "$second_month"
    success "Future allowance revoked back to the vault."

    advance_calendar_to "$second_unlock" "Fast-forward to the revoked allowance month"

    step "Attempt the revoked monthly allowance"
    expect_failure "the authorization conflicts with the already-confirmed revocation" \
        "$MAIN" phone authorize "$second_month"
    success "Revoked allowance remained unspendable."
}

test_partial_funding() {
    setup_vault 350000
    step "Run rollover with only enough funds for the earliest allowances"
    ceremony "$NOW" 100000
    confirm_transaction "Confirming the partial annual rollover"
    status_compact
    success "Earliest three allowances funded; rollover continued."
}

test_lost_phone() {
    local old_address new_address first_month first_unlock
    setup_vault

    step "Approve the annual monthly-spending policy before the phone is lost"
    ceremony "$NOW"
    confirm_transaction "Confirming the annual rollover"
    success "Twelve monthly transaction pairs presigned."

    old_address=$(jq -r .vault_address "$MAIN/vault.json")

    step "Simulate losing the phone"
    show_file_command rm -- "$MAIN/phone/device.json"
    rm -- "$MAIN/phone/device.json"
    expect_failure "the hot wallet cannot open without its phone key" \
        "$MAIN" phone receive-address
    success "Phone loss detected; wallet access blocked."

    step "Use the HWW to decrypt the cloud backup, then rotate the phone key"
    restore_phone "$MAIN"
    rotate_phone_key "$MAIN"
    new_address=$(jq -r .vault_address "$MAIN/vault.json")
    if [[ $old_address == "$new_address" ]]; then
        printf 'ERROR: emergency key rotation reused the vault address\n' >&2
        exit 1
    fi
    confirm_transaction "Confirming emergency key rotation"
    vault "$MAIN" status
    if [[ $(jq -r '.monthly_limit_sats' "$MAIN/vault.json") != "$DEFAULT_MONTHLY_LIMIT_SATS" ]]; then
        printf 'ERROR: phone-key rotation did not preserve the monthly limit\n' >&2
        exit 1
    fi
    if [[ $(jq '.entries | length' "$MAIN/phone/schedule.json") -ne 12 ]]; then
        printf 'ERROR: phone-key rotation did not create a replacement annual schedule\n' >&2
        exit 1
    fi
    success "Phone restored, key rotated, and monthly policy preserved."

    first_month=$(jq -r '.entries[0].month' "$MAIN/phone/schedule.json")
    first_unlock=$(jq -r '.entries[0].unlock_timestamp' "$MAIN/phone/schedule.json")
    advance_calendar_to "$first_unlock" "Fast-forward to the replacement phone's first allowance"
    step "Use the monthly allowance preserved through rotation"
    vault "$MAIN" phone authorize "$first_month"
    confirm_transaction "Confirming the replacement phone's monthly authorization"
    success "Replacement phone used the preserved monthly policy."
}

test_stolen_phone() {
    local attacker_dir="$DEMO_ROOT/stolen-phone-attacker"
    local attacker_address
    setup_vault
    make_receiver attacker_address "Attacker"

    step "Simulate theft of the phone key and hot-wallet state"
    show_file_command cp -a "$MAIN" "$attacker_dir"
    cp -a "$MAIN" "$attacker_dir"
    show_file_command rm -- "$attacker_dir/hww/device.json"
    rm -- "$attacker_dir/hww/device.json"
    vault "$attacker_dir" phone send "$attacker_address" 500000
    vault "$attacker_dir" phone create-sweep "$attacker_address" \
        --output "${E2E_TEST}-attacker-sweep.json"
    expect_failure "the stolen phone lacks H for an immediate cold-vault sweep" \
        "$attacker_dir" hww confirm-sweep "${E2E_TEST}-attacker-sweep.json" \
        --output "${E2E_TEST}-attacker-approved-sweep.json" --yes
    expect_failure "the stolen phone recovery path is not mature" \
        "$attacker_dir" phone recover "$attacker_address"
    success "Hot funds exposed; cold vault remained protected."

    step "The owner restores the phone backup with the HWW and rotates immediately"
    show_file_command rm -- "$MAIN/phone/device.json"
    rm -- "$MAIN/phone/device.json"
    restore_phone "$MAIN"
    rotate_phone_key "$MAIN"
    confirm_transaction "Confirming emergency key rotation"
    vault "$MAIN" status
    success "Owner restored and rotated away from stolen key."
}

test_lost_hww() {
    local month unlock latest_height recovery_address target
    setup_vault

    step "Presign the annual schedule before the HWW is lost"
    ceremony "$NOW"
    confirm_transaction "Confirming the annual rollover"
    month=$(jq -r '.entries[0].month' "$MAIN/phone/schedule.json")
    unlock=$(jq -r '.entries[0].unlock_timestamp' "$MAIN/phone/schedule.json")
    show_file_command rm -- "$MAIN/hww/device.json"
    rm -- "$MAIN/hww/device.json"
    success "Annual schedule presigned before HWW loss."

    advance_calendar_to "$unlock" "Fast-forward to a presigned monthly allowance"

    step "Use the phone-held allowance without the HWW"
    vault "$MAIN" phone authorize "$month"
    confirm_transaction "Confirming the phone-held monthly authorization"
    vault_capture recovery_address "$MAIN" phone receive-address
    recovery_address=$(printf '%s\n' "$recovery_address" | awk '/^Hot receive address:/ {print $4}')
    latest_height=$(node_height)
    target=$((latest_height + PHONE_RECOVERY_BLOCKS))
    expect_failure "phone-only recovery is not mature yet" \
        "$MAIN" phone recover "$recovery_address"
    success "Monthly spend worked; early recovery stayed locked."

    step "Wait for phone-only recovery"
    mine_to_next_height "$target" "Mining the real 61,200-block phone recovery delay"
    vault "$MAIN" phone recover "$recovery_address"
    confirm_transaction "Confirming the phone-only recovery sweep"
    status_compact
    success "Phone recovered the full remaining vault balance."
}

test_stolen_hww() {
    local attacker_dir="$DEMO_ROOT/stolen-hww-attacker"
    local owner_address attacker_address phone_target hww_target
    setup_vault
    make_receiver attacker_address "Attacker"
    vault_capture owner_address "$MAIN" phone receive-address
    owner_address=$(printf '%s\n' "$owner_address" | awk '/^Hot receive address:/ {print $4}')

    step "Simulate HWW theft: the attacker has H and the owner retains M"
    show_file_command cp -a "$MAIN" "$attacker_dir"
    cp -a "$MAIN" "$attacker_dir"
    show_file_command rm -- "$attacker_dir/phone/device.json"
    rm -- "$attacker_dir/phone/device.json"
    show_file_command rm -- "$MAIN/hww/device.json"
    rm -- "$MAIN/hww/device.json"
    expect_failure "HWW recovery is still inside the phone-priority window" \
        "$attacker_dir" hww recover "$attacker_address"
    success "Stolen HWW blocked during phone-priority window."

    phone_target=$((FUNDING_HEIGHT + PHONE_RECOVERY_BLOCKS))
    step "The legitimate phone reaches its earlier recovery window"
    mine_to_next_height "$phone_target" "Mining the real 61,200-block phone recovery delay"
    vault "$MAIN" phone recover "$owner_address"
    confirm_transaction "Confirming the legitimate phone recovery sweep"
    success "Legitimate phone recovered funds first."

    hww_target=$((FUNDING_HEIGHT + HWW_RECOVERY_BLOCKS))
    step "The stolen HWW eventually reaches its later recovery window"
    mine_to_next_height "$hww_target" "Mining the remaining blocks to HWW recovery"
    expect_failure "the legitimate phone already spent the old vault output" \
        "$attacker_dir" hww recover "$attacker_address"
    success "Stolen HWW reached maturity too late."
}

test_lost_phone_no_cloud() {
    local recovery_address target
    setup_vault
    make_receiver recovery_address "Recovered owner"

    step "Simulate losing the phone and its cloud backup"
    show_file_command rm -- "$MAIN/phone/device.json" "$MAIN/cloud/phone-seed-backup.json"
    rm -- "$MAIN/phone/device.json" "$MAIN/cloud/phone-seed-backup.json"
    expect_failure "there is no encrypted phone backup to restore" \
        "$MAIN" hww decrypt-phone-backup "$MAIN/cloud/phone-seed-backup.json" \
        --output "${E2E_TEST}-missing-recovery.json"
    expect_failure "the HWW fallback is not mature yet" \
        "$MAIN" hww recover "$recovery_address"
    success "Missing backup confirmed; early HWW recovery blocked."

    target=$((FUNDING_HEIGHT + HWW_RECOVERY_BLOCKS))
    step "Wait for HWW-only recovery"
    mine_to_next_height "$target" "Mining the real 65,535-block HWW recovery delay"
    vault "$MAIN" hww recover "$recovery_address"
    confirm_transaction "Confirming the HWW-only recovery sweep"
    status_compact
    success "Surviving HWW recovered the vault after maturity."
}

test_both_lost() {
    local recovery_address target
    setup_vault
    make_receiver recovery_address "Replacement owner"

    step "Simulate losing both devices while encrypted cloud data survives"
    show_file_command rm -- "$MAIN/phone/device.json" "$MAIN/hww/device.json"
    rm -- "$MAIN/phone/device.json" "$MAIN/hww/device.json"
    expect_failure "the backup cannot be decrypted without the HWW" \
        "$MAIN" hww decrypt-phone-backup "$MAIN/cloud/phone-seed-backup.json" \
        --output "${E2E_TEST}-missing-hww-recovery.json"
    success "Both device keys lost; backup cannot be decrypted."

    target=$((FUNDING_HEIGHT + HWW_RECOVERY_BLOCKS))
    step "Wait until every consensus recovery path is mature"
    mine_to_next_height "$target" "Mining the real 65,535-block HWW recovery delay"
    expect_failure "no surviving key can sign either mature recovery path" \
        "$MAIN" hww recover "$recovery_address"
    printf 'No recovery friend was configured for this vault, so no surviving key exists.\n'
    success "Mature funds remain unrecoverable without a key."
}

test_cloud_compromise() {
    local attacker_dir="$DEMO_ROOT/cloud-attacker"
    local attacker_address
    setup_vault
    make_receiver attacker_address "Attacker"

    step "Simulate theft of only the encrypted cloud backup"
    show_file_command mkdir -p "$attacker_dir/cloud"
    mkdir -p "$attacker_dir/cloud"
    show_file_command cp "$MAIN/vault.json" "$attacker_dir/vault.json"
    cp "$MAIN/vault.json" "$attacker_dir/vault.json"
    show_file_command cp "$MAIN/cloud/phone-seed-backup.json" "$attacker_dir/cloud/phone-seed-backup.json"
    cp "$MAIN/cloud/phone-seed-backup.json" "$attacker_dir/cloud/phone-seed-backup.json"
    expect_failure "cloud ciphertext alone cannot restore the phone" \
        "$attacker_dir" hww decrypt-phone-backup \
        "$attacker_dir/cloud/phone-seed-backup.json" \
        --output "${E2E_TEST}-cloud-recovery.json"
    expect_failure "cloud ciphertext alone cannot sign a cooperative sweep" \
        "$attacker_dir" phone create-sweep "$attacker_address" \
        --output "${E2E_TEST}-cloud-sweep.json"
    success "Cloud ciphertext revealed no spending capability."
}

test_social_recovery() {
    local friend_public="$DEMO_ROOT/alice-recovery.pub.asc"
    local friend_private="$DEMO_ROOT/alice-recovery.sec.asc"
    local recovery_package="$DEMO_ROOT/alice-phone-recovery.json"
    local recovery_address target
    setup_vault
    make_receiver recovery_address "Replacement owner"

    step "Create a recovery friend's OpenPGP key and add it with the HWW"
    vault "$MAIN" social generate-friend-key --name "Alice <alice@example.test>" \
        --public-key "$friend_public" --private-key "$friend_private"
    vault "$MAIN" hww add-recovery-friend "$friend_public" --yes
    printf 'Cloud backup contains one encrypted payload, one HWW key wrapper, and %s OpenPGP friend wrapper.\n' \
        "$(jq '.friends | length' "$MAIN/cloud/phone-seed-backup.json")"
    success "Phone key and descriptor protected for HWW and recovery friend."

    step "Lose both devices and decrypt the cloud backup with the friend key"
    show_file_command rm -- "$MAIN/phone/device.json" "$MAIN/hww/device.json"
    rm -- "$MAIN/phone/device.json" "$MAIN/hww/device.json"
    vault "$MAIN" social decrypt-backup "$MAIN/cloud/phone-seed-backup.json" \
        --private-key "$friend_private" --output "$recovery_package"
    expect_failure "social recovery reconstructs M but cannot bypass its on-chain delay" \
        "$MAIN" social emergency-access "$MAIN/cloud/phone-seed-backup.json" \
        --private-key "$friend_private" "$recovery_address"
    success "Friend decrypted the backup; early emergency access stayed locked."

    target=$((FUNDING_HEIGHT + PHONE_RECOVERY_BLOCKS))
    step "Wait for delayed social emergency access"
    mine_to_next_height "$target" "Mining the real 61,200-block phone recovery delay"
    vault "$MAIN" social emergency-access "$MAIN/cloud/phone-seed-backup.json" \
        --private-key "$friend_private" "$recovery_address"
    confirm_transaction "Confirming the social emergency-access sweep"
    status_compact
    success "Recovery friend swept the vault after the phone delay matured."
}

test_both_compromised() {
    local attacker_dir="$DEMO_ROOT/both-keys-attacker"
    local attacker_address
    setup_vault
    make_receiver attacker_address "Attacker"

    step "Simulate compromise of both phone and HWW keys"
    show_file_command cp -a "$MAIN" "$attacker_dir"
    cp -a "$MAIN" "$attacker_dir"
    cooperative_sweep "$attacker_dir" "$attacker_address"
    confirm_transaction "Confirming the attacker's immediate cooperative sweep"
    status_compact
    success "Both compromised keys drained the vault immediately."
}

test_rollover_on_time() {
    local initial_rollover_height annual_next_height pre_calendar_target anniversary
    local old_phone_target old_month old_schedule current_mtp
    local renewed_rollover_height renewed_oldest renewed_phone_target
    setup_vault

    step "Create the first annual schedule"
    ceremony "$NOW"
    confirm_transaction "Confirming the first annual rollover"
    initial_rollover_height=$(node_height)
    old_phone_target=$((initial_rollover_height + PHONE_RECOVERY_BLOCKS))
    old_month=$(jq -r '.entries[0].month' "$MAIN/phone/schedule.json")
    old_schedule="$DEMO_ROOT/rollover-on-time-old-schedule"
    cp -a "$MAIN" "$old_schedule"
    vault "$MAIN" status
    success "Initial annual rollover confirmed with twelve monthly pairs."

    annual_next_height=$((initial_rollover_height + BLOCKS_PER_YEAR))
    pre_calendar_target=$((annual_next_height - 11))
    anniversary=$((NOW + 365 * 24 * 60 * 60))
    step "Wait one year without crossing either recovery deadline"
    mine_to_next_height "$pre_calendar_target" \
        "Mining one year of real block height for the scheduled rollover"
    advance_calendar_to "$anniversary" "Advance the calendar to the annual rollover date"
    vault "$MAIN" status
    expect_failure "phone recovery is still locked at the scheduled rollover" \
        "$MAIN" phone recover "$MINING_ADDRESS"
    expect_failure "HWW recovery is still locked at the scheduled rollover" \
        "$MAIN" hww recover "$MINING_ADDRESS"
    success "Annual rollover date reached before either recovery path activated."

    step "Perform the scheduled cooperative rollover"
    current_mtp=$(node_mtp)
    ceremony "$current_mtp"
    confirm_transaction "Confirming the on-time annual rollover"
    renewed_rollover_height=$(node_height)
    vault "$MAIN" status
    renewed_oldest=$(command vault --data-dir "$MAIN" status | \
        awk '/^Oldest confirmation height:/ {print $4}')
    renewed_phone_target=$(command vault --data-dir "$MAIN" status | \
        awk '/^Phone recovery: valid next-block height/ {print $6}')
    if [[ $renewed_oldest != "$renewed_rollover_height" || \
        $renewed_phone_target != "$((renewed_rollover_height + PHONE_RECOVERY_BLOCKS))" ]]; then
        printf 'ERROR: on-time rollover did not reset the recovery timers\n' >&2
        exit 1
    fi
    success "All funds rolled over and both recovery timers reset."

    step "Try a retained authorization from the previous annual schedule"
    expect_failure "the confirmed rollover spent every old monthly chunk" \
        "$old_schedule" phone authorize "$old_month"
    success "Old monthly authorizations were invalidated by the rollover."

    step "Reach the old phone-recovery deadline"
    mine_to_next_height "$old_phone_target" \
        "Mining until the previous schedule's phone-recovery deadline"
    vault "$MAIN" status
    expect_failure "the renewed outputs have a fresh phone-recovery delay" \
        "$MAIN" phone recover "$MINING_ADDRESS"
    success "Old recovery deadline passed while renewed funds stayed locked."
}

test_rollover_forgotten() {
    local phone_target hww_target current_mtp
    setup_vault

    step "Forget the annual rollover and inspect the original vault output"
    vault "$MAIN" status

    phone_target=$((FUNDING_HEIGHT + PHONE_RECOVERY_BLOCKS))
    mine_to_next_height "$phone_target" "Mining until phone-only recovery activates"
    vault "$MAIN" status

    hww_target=$((FUNDING_HEIGHT + HWW_RECOVERY_BLOCKS))
    mine_to_next_height "$hww_target" "Mining until HWW-only recovery also activates"
    vault "$MAIN" status
    success "Forgotten rollover exposed both recovery paths."

    step "Perform a late cooperative rollover to renew both timers"
    current_mtp=$(node_mtp)
    ceremony "$current_mtp"
    confirm_transaction "Confirming the late annual rollover"
    vault "$MAIN" status
    success "Late rollover renewed both recovery timers."
}

if (( $(node_height) != 0 )); then
    printf 'ERROR: test %s requires a fresh height-zero regtest node.\n' "$E2E_TEST" >&2
    printf 'Run it through ./scripts/run-e2e.sh so the node is reset first.\n' >&2
    exit 1
fi

printf 'Started at %s. All keys and funds are disposable regtest data.\n' \
    "$(date -u '+%Y-%m-%d %H:%M:%S UTC')"

case "$E2E_TEST" in
    setup-policy) test_setup_policy ;;
    monthly-spend) test_monthly_spend ;;
    monthly-revoke) test_monthly_revoke ;;
    partial-funding) test_partial_funding ;;
    lost-phone) test_lost_phone ;;
    stolen-phone) test_stolen_phone ;;
    lost-hww) test_lost_hww ;;
    stolen-hww) test_stolen_hww ;;
    lost-phone-no-cloud) test_lost_phone_no_cloud ;;
    both-lost) test_both_lost ;;
    cloud-compromise) test_cloud_compromise ;;
    social-recovery) test_social_recovery ;;
    both-compromised) test_both_compromised ;;
    rollover-on-time) test_rollover_on_time ;;
    rollover-forgotten) test_rollover_forgotten ;;
esac

printf '\n✨ Test passed: %s\n' "$E2E_TEST"
