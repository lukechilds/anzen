#!/usr/bin/env bash
set -euo pipefail

readonly DEMO_ROOT=/demo
readonly DEFAULT_MONTHLY_LIMIT_SATS=10000000
readonly DEFAULT_FUNDING_SATS=200000000
readonly HWW_RECOVERY_SECONDS=33553920
readonly MONTHLY_ALLOWANCE_DELAY_SECONDS=2592256
readonly EMERGENCY_ACCESS_DELAY_SECONDS=605184

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
    emergency-access
    emergency-cancel
    hww-revoke
    partial-funding
    lost-phone
    stolen-phone
    lost-hww
    stolen-hww
    lost-phone-no-cloud
    both-lost
    cloud-compromise
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
# BIP68 counts a coin's time lock from the median time past of the block before its own.
FUNDING_START=

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
    printf '$ anzen'
    printf ' %q' "$@"
    printf '\n'
}

show_file_command() {
    printf '$'
    printf ' %q' "$@"
    printf '\n'
}

anzen() {
    local data_dir=$1
    local output
    local anzen_exit
    shift
    show_command "$data_dir" "$@"
    set +e
    output=$(command anzen --data-dir "$data_dir" "$@" 2>&1)
    anzen_exit=$?
    set -e
    print_cli_output "$output"
    return "$anzen_exit"
}

anzen_capture() {
    local variable_name=$1
    local data_dir=$2
    local output
    shift 2
    show_command "$data_dir" "$@"
    if ! output=$(command anzen --data-dir "$data_dir" "$@" 2>&1); then
        print_cli_output "$output"
        return 1
    fi
    print_cli_output "$output"
    printf -v "$variable_name" '%s' "$output"
}

anzen_silent() {
    local data_dir=$1
    local output
    local status
    shift
    set +e
    output=$(command anzen --data-dir "$data_dir" "$@" 2>&1)
    status=$?
    set -e
    if [[ $status -ne 0 ]]; then
        print_cli_output "$output" >&2
        return "$status"
    fi
}

anzen_filtered() {
    local filter=$1
    local data_dir=$2
    local output
    shift 2
    show_command "$data_dir" "$@"
    if ! output=$(command anzen --data-dir "$data_dir" "$@" 2>&1); then
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
    output=$(command anzen --data-dir "$data_dir" "$@" 2>&1)
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
    command anzen --data-dir "$MAIN" node info | awk '/^Height:/ {print $2}'
}

node_mtp() {
    command anzen --data-dir "$MAIN" node info | awk '/^Median time past:/ {print $4}'
}

live_controller_count() {
    command anzen --data-dir "$MAIN" status | \
        awk '/^Live policy controller outputs:/ {print $5}'
}

vault_balance_sats() {
    command anzen --data-dir "$MAIN" status | \
        awk '/^Vault balance:/ {print $3}'
}

require_live_controllers() {
    local expected=$1
    local actual
    actual=$(live_controller_count)
    if [[ $actual != "$expected" ]]; then
        printf 'ERROR: expected %s live policy controller output(s), found %s\n' \
            "$expected" "$actual" >&2
        exit 1
    fi
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
        anzen_silent "$MAIN" node mine "$blocks" "$MINING_ADDRESS"
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
            anzen_silent "$MAIN" node set-time "$((mtp + batch + 60))"
        fi
        anzen_silent "$MAIN" node mine "$batch" "$MINING_ADDRESS"
        completed=$((completed + batch))
        mining_progress "$completed" "$blocks" "$started_at"
    done
    printf '\nReached block height %s.\n' "$(node_height)"
}

advance_mtp_to() {
    local target=$1
    local label=$2
    local target_display
    target_display=$(date -u -d "@$target" '+%Y-%m-%d %H:%M:%S UTC')
    step "$label"
    show_command "$MAIN" node set-time "$((target + 60))"
    anzen_silent "$MAIN" node set-time "$((target + 60))"
    show_command "$MAIN" node mine 11 "$MINING_ADDRESS"
    anzen_silent "$MAIN" node mine 11 "$MINING_ADDRESS"
    if (( $(node_mtp) <= target )); then
        printf 'ERROR: Median Time Past did not advance beyond %s\n' "$target_display" >&2
        exit 1
    fi
    printf 'Regtest Median Time Past is now beyond %s.\n' "$target_display"
    success "$label complete."
}

# Days until the HWW-alone path opens, as `anzen status` reports it; 0 once it is active.
hww_days_remaining() {
    command anzen --data-dir "$MAIN" status | sed -n \
        -e 's/^HWW recovery: ACTIVE.*/0/p' \
        -e 's/^HWW recovery: opens at .*(\([0-9]*\) days remaining)$/\1/p'
}

init_vault() {
    anzen_filtered '
        /^(Simulated phone initialized|Phone mnemonic:|Phone vault key:)/ { print }
    ' "$MAIN" phone init
    anzen_filtered '
        /^(Simulated HWW initialized|HWW mnemonic:|HWW vault key:|HWW ready)/ { print }
    ' "$MAIN" hww init
    anzen_filtered '
        /^(Vault initialized|Cold storage descriptor:|Vault address:|HWW alone:|Monthly spending:|Emergency access:|Cloud recovery backup:)/ { print }
    ' "$MAIN" init
}

show_backup_metadata() {
    printf 'Encrypted cloud payload: random symmetric key, %s-byte nonce, %s-byte ciphertext; key wrapped for the HWW only.\n' \
        "$(jq -r '.encrypted_payload.nonce | length' "$MAIN/cloud/phone-seed-backup.json")" \
        "$(jq -r '.encrypted_payload.ciphertext | length' "$MAIN/cloud/phone-seed-backup.json")"
}

ceremony() {
    local now=$1
    local monthly_limit=${2:-$DEFAULT_MONTHLY_LIMIT_SATS}
    local emergency_access_limit=${3:-0}
    local proposal="${E2E_TEST}-policy.json"
    local approved="${E2E_TEST}-approved-policy.json"
    anzen_filtered '
        /^(PHONE POLICY PROPOSAL|Cold storage descriptor:|Vault address:|Policy controller address:|Policy controller reserve:|Monthly spending:|Monthly limit:|Emergency access:|Emergency access limit:|Emergency access delay:|Fee rate:|Total input:|Allowance steps:|Allowance hop delay:|WARNING:|Rollover txid:|Rollover fee:|Initial allowance-chain UTXO:|Rollover remainder:|Emergency trigger txid:|Emergency withdrawal txid:|Emergency hot address:|Presigned transactions:|Policy proposal:)/ { print }
    ' "$MAIN" phone set-policy --monthly-limit "$monthly_limit" \
        --emergency-access-limit "$emergency_access_limit" \
        --output "$proposal" --now "$now"
    anzen_filtered '
        /^(SIMULATED HWW|Policy controller address:|Policy controller reserve:|Monthly spending:|Monthly limit:|Emergency access:|Emergency access limit:|Emergency access delay:|Allowance steps:|Allowance hop delay:|Rollover txid:|Initial allowance-chain UTXO:|Rollover remainder:|Emergency trigger txid:|Emergency withdrawal txid:|Emergency hot address:|Presigned transactions:|WARNING:|HWW validated and signed|HWW-approved policy:)/ { print }
    ' "$MAIN" hww confirm-policy "$proposal" --output "$approved" --yes
    anzen_filtered '
        /^(Rollover broadcast:|Active monthly limit:|Encrypted allowance authorizations:|Active emergency access:|Encrypted emergency transaction set:|Emergency access:)/ { print }
    ' "$MAIN" phone activate-policy "$approved"
}

status_compact() {
    anzen_filtered '
        /^(Network:|Height:|Vault UTXOs:|Vault balance:|Live policy controller outputs:)/ { print }
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
    anzen_capture hot_output "$MAIN" phone receive-address
    MINING_ADDRESS=$(printf '%s\n' "$hot_output" | awk '/^Hot receive address:/ {print $4}')
    anzen "$MAIN" node set-time "$NOW"
    success "Phone, HWW, hot wallet, and vault configured."

    step "Mine spendable regtest coins and fund the vault"
    mine_blocks 101 "Creating spendable regtest coins"
    vault_address=$(jq -r .vault_address "$MAIN/anzen.json")
    anzen "$MAIN" phone send "$vault_address" "$funding_sats"
    FUNDING_START=$(node_mtp)
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
    command anzen --data-dir "$receiver_dir" phone init >/dev/null
    command anzen --data-dir "$receiver_dir" hww init >/dev/null
    command anzen --data-dir "$receiver_dir" init >/dev/null
    anzen_capture receiver_output "$receiver_dir" phone receive-address
    printf -v "$variable_name" '%s' "$(printf '%s\n' "$receiver_output" | awk '/^Hot receive address:/ {print $4}')"
    success "$label receiving address ready."
}

confirm_transaction() {
    mine_blocks 1 "$1"
}

# A replacement phone starts empty. The HWW exports public vault state, then releases the cloud
# backup's key sealed to the replacement's fresh key; the HWW never sees the phone seed.
restore_phone() {
    local data_dir=$1
    local bundle="${E2E_TEST}-recovery-bundle.json"
    local request="${E2E_TEST}-restore-request.json"
    local response="${E2E_TEST}-restore-response.json"
    show_file_command rm -rf -- "$data_dir/phone" "$data_dir/anzen.json"
    rm -rf -- "$data_dir/phone" "$data_dir/anzen.json"
    anzen_filtered '
        /^(Simulated phone initialized|Phone vault key:)/ { print }
    ' "$data_dir" phone init
    anzen "$data_dir" hww export-recovery --output "$bundle"
    anzen "$data_dir" phone import-recovery "$bundle"
    anzen "$data_dir" phone restore-request \
        "$data_dir/cloud/phone-seed-backup.json" --output "$request"
    anzen "$data_dir" hww approve-restore "$request" --output "$response" --yes
    anzen "$data_dir" phone restore "$response"
}

rotate_phone_key() {
    local data_dir=$1
    local proposal="${E2E_TEST}-rotation.json"
    local approved="${E2E_TEST}-approved-rotation.json"
    anzen "$data_dir" phone rotate-key --output "$proposal"
    anzen "$data_dir" hww confirm-rotation "$proposal" --output "$approved" --yes
    anzen "$data_dir" phone activate-rotation "$approved"
    anzen "$data_dir" hww adopt-rotation "$approved"
}

cooperative_sweep() {
    local data_dir=$1
    local destination=$2
    local proposal="${E2E_TEST}-sweep.json"
    local approved="${E2E_TEST}-approved-sweep.json"
    anzen "$data_dir" phone create-sweep "$destination" --output "$proposal"
    anzen "$data_dir" hww confirm-sweep "$proposal" --output "$approved" --yes
    anzen "$data_dir" phone broadcast-sweep "$approved"
}

test_setup_policy() {
    NOW=$(date -u +%s)
    step "Set up the simulated phone, HWW, hot wallet, and static vault"
    init_vault
    show_backup_metadata
    anzen "$MAIN" policy
    printf 'Allowance delays begin from each chain output confirmation, not from a calendar date.\n'
    success "Vault policy configured and verified."
}

test_monthly_spend() {
    local first_delay_base first_unlock second_delay_base second_unlock
    setup_vault

    step "Approve one annual policy and presign the allowance chain"
    ceremony "$NOW"
    first_delay_base=$(node_mtp)
    confirm_transaction "Confirming the annual rollover"
    first_unlock=$((first_delay_base + MONTHLY_ALLOWANCE_DELAY_SECONDS))
    printf 'Step 1 becomes valid about 30 days after the rollover confirms.\n'
    printf 'Each authorization is encrypted as an incomplete PSBT; the phone signs its controller input only when executing it.\n'
    success "Twelve sequential allowance authorizations presigned."

    step "Attempt the first allowance before it unlocks"
    expect_failure "step 1 has not completed its relative 30-day delay" \
        "$MAIN" phone authorize 1
    success "Pre-unlock allowance correctly rejected."

    advance_mtp_to "$first_unlock" "Fast-forward through the first allowance delay"

    step "Execute the allowance with a lower soft limit"
    anzen "$MAIN" phone authorize 1
    second_delay_base=$(node_mtp)
    anzen "$MAIN" phone apply-soft-limit 1 --limit 1000000
    confirm_transaction "Confirming the authorization and soft-limit return"
    printf 'The 0.1 BTC monthly allowance retained 0.01 BTC hot and returned 0.09 BTC to cold storage (fees paid from hot funds).\n'
    status_compact
    success "0.01 BTC retained; 0.09 BTC returned cold."

    step "Attempt the second hop immediately after the first"
    expect_failure "each new chain output starts its own relative 30-day delay" \
        "$MAIN" phone authorize 2
    success "Step 2 did not inherit step 1's elapsed time."

    second_unlock=$((second_delay_base + MONTHLY_ALLOWANCE_DELAY_SECONDS))
    advance_mtp_to "$second_unlock" "Fast-forward through the second allowance delay"
    step "Execute the second sequential allowance"
    anzen "$MAIN" phone authorize 2
    confirm_transaction "Confirming the second allowance authorization"
    success "Two sequential allowance hops executed."
}

test_monthly_revoke() {
    local first_delay_base first_unlock second_delay_base second_unlock revoke_time
    local balance_before balance_after
    setup_vault

    step "Approve one annual policy and presign the allowance chain"
    ceremony "$NOW"
    first_delay_base=$(node_mtp)
    confirm_transaction "Confirming the annual rollover"
    first_unlock=$((first_delay_base + MONTHLY_ALLOWANCE_DELAY_SECONDS))
    success "Twelve sequential allowance authorizations presigned."

    advance_mtp_to "$first_unlock" "Fast-forward through the first allowance delay"
    step "Execute step 1 so step 2 becomes the live chain output"
    anzen "$MAIN" phone authorize 1
    second_delay_base=$(node_mtp)
    confirm_transaction "Confirming the first allowance authorization"
    success "Step 1 released funds and created the step 2 output."

    revoke_time=$((second_delay_base + 14 * 24 * 60 * 60))
    advance_mtp_to "$revoke_time" "Fast-forward two weeks into step 2's delay"
    step "Revoke the entire remaining allowance chain from the phone"
    balance_before=$(vault_balance_sats)
    anzen "$MAIN" phone revoke 2
    confirm_transaction "Confirming the whole-chain revocation"
    require_live_controllers 0
    balance_after=$(vault_balance_sats)
    if [[ $balance_after != "$balance_before" ]]; then
        printf 'ERROR: monthly revocation moved vault principal (%s -> %s sats)\n' \
            "$balance_before" "$balance_after" >&2
        exit 1
    fi
    success "The live controller was spent; cold principal remained in the vault."

    second_unlock=$((second_delay_base + MONTHLY_ALLOWANCE_DELAY_SECONDS))
    advance_mtp_to "$second_unlock" "Fast-forward beyond step 2's original delay"

    step "Attempt revoked current and future allowance hops"
    expect_failure "step 2 conflicts with the confirmed whole-chain revocation" \
        "$MAIN" phone authorize 2
    expect_failure "step 3 depended on the now-impossible step 2 transaction" \
        "$MAIN" phone authorize 3
    success "Whole-chain revocation permanently invalidated all remaining hops."
}

test_emergency_access() {
    local relative_lock_base unlock_time
    setup_vault

    step "Approve monthly spending and one emergency withdrawal for this vault epoch"
    ceremony "$NOW" "$DEFAULT_MONTHLY_LIMIT_SATS" 50000000
    confirm_transaction "Confirming the annual rollover"
    success "Monthly policy and 0.5 BTC emergency access presigned."

    step "Start emergency access from the phone"
    relative_lock_base=$(node_mtp)
    anzen "$MAIN" phone emergency initiate
    confirm_transaction "Confirming the emergency trigger"
    expect_failure "the emergency withdrawal is locked during its cancellation window" \
        "$MAIN" phone emergency withdraw
    success "Emergency access started; withdrawal stayed locked."

    unlock_time=$((relative_lock_base + EMERGENCY_ACCESS_DELAY_SECONDS))
    advance_mtp_to "$unlock_time" "Fast-forward through the one-week cancellation window"

    step "Withdraw the preconfigured emergency amount to the hot wallet"
    anzen "$MAIN" phone emergency withdraw
    confirm_transaction "Confirming the delayed emergency withdrawal"
    status_compact
    success "0.5 BTC emergency access reached the hot wallet."
}

test_emergency_cancel() {
    local relative_lock_base unlock_time balance_before balance_after
    setup_vault

    step "Approve one cancellable emergency withdrawal for this vault epoch"
    ceremony "$NOW" "$DEFAULT_MONTHLY_LIMIT_SATS" 50000000
    confirm_transaction "Confirming the annual rollover"
    success "One-week delayed emergency access presigned."

    step "Start emergency access from the phone"
    relative_lock_base=$(node_mtp)
    anzen "$MAIN" phone emergency initiate
    confirm_transaction "Confirming the emergency trigger"
    expect_failure "the emergency withdrawal is still inside its cancellation window" \
        "$MAIN" phone emergency withdraw
    success "Emergency access started with withdrawal still locked."

    step "Cancel emergency access from the phone before it unlocks"
    balance_before=$(vault_balance_sats)
    anzen "$MAIN" phone emergency cancel
    confirm_transaction "Confirming the emergency cancellation"
    require_live_controllers 1
    balance_after=$(vault_balance_sats)
    if [[ $balance_after != "$balance_before" ]]; then
        printf 'ERROR: emergency cancellation moved staged vault principal (%s -> %s sats)\n' \
            "$balance_before" "$balance_after" >&2
        exit 1
    fi
    success "The withdrawal controller was spent; staged funds remained in the vault."

    unlock_time=$((relative_lock_base + EMERGENCY_ACCESS_DELAY_SECONDS))
    advance_mtp_to "$unlock_time" "Fast-forward beyond the cancelled withdrawal's delay"

    step "Attempt the cancelled emergency withdrawal"
    expect_failure "the cancellation already spent the withdrawal controller" \
        "$MAIN" phone emergency withdraw
    status_compact
    success "Cancelled emergency access remained unspendable."
}

test_hww_revoke() {
    local first_delay_base first_unlock safe_address balance_before balance_after
    setup_vault

    step "Approve monthly and emergency policy actions"
    ceremony "$NOW" "$DEFAULT_MONTHLY_LIMIT_SATS" 50000000
    first_delay_base=$(node_mtp)
    confirm_transaction "Confirming the annual rollover"
    status_compact
    success "Both independent policy controllers became live."

    make_receiver safe_address "Policy-revocation recipient"
    step "Revoke every live policy state from the HWW"
    balance_before=$(vault_balance_sats)
    anzen "$MAIN" hww revoke-policy "$safe_address" --yes
    confirm_transaction "Confirming the HWW policy revocation"
    require_live_controllers 0
    balance_after=$(vault_balance_sats)
    if [[ $balance_after != "$balance_before" ]]; then
        printf 'ERROR: HWW policy revocation moved vault principal (%s -> %s sats)\n' \
            "$balance_before" "$balance_after" >&2
        exit 1
    fi
    status_compact
    success "HWW spent both controllers to a verified safe destination."

    first_unlock=$((first_delay_base + MONTHLY_ALLOWANCE_DELAY_SECONDS))
    advance_mtp_to "$first_unlock" "Fast-forward beyond the first allowance delay"
    step "Attempt actions from the now-revoked phone policy"
    expect_failure "the HWW already spent the monthly controller" \
        "$MAIN" phone authorize 1
    expect_failure "the HWW already spent the emergency controller" \
        "$MAIN" phone emergency initiate
    success "A surviving HWW invalidated every phone-held policy action."
}

test_partial_funding() {
    setup_vault 350000
    step "Run rollover with only enough funds for the earliest allowances"
    ceremony "$NOW" 100000
    confirm_transaction "Confirming the partial annual rollover"
    status_compact
    success "Three sequential allowances funded; rollover continued."
}

test_lost_phone() {
    local old_address new_address first_delay_base first_unlock
    setup_vault

    step "Approve the annual vault policy before the phone is lost"
    ceremony "$NOW" "$DEFAULT_MONTHLY_LIMIT_SATS" 50000000
    confirm_transaction "Confirming the annual rollover"
    success "Monthly and emergency policy transactions presigned."

    old_address=$(jq -r .vault_address "$MAIN/anzen.json")

    step "Simulate losing the phone"
    show_file_command rm -- "$MAIN/phone/device.json"
    rm -- "$MAIN/phone/device.json"
    expect_failure "the hot wallet cannot open without its phone key" \
        "$MAIN" phone receive-address
    success "Phone loss detected; wallet access blocked."

    step "Restore the phone key through the HWW, then rotate to the replacement key"
    restore_phone "$MAIN"
    rotate_phone_key "$MAIN"
    first_delay_base=$(node_mtp)
    new_address=$(jq -r .vault_address "$MAIN/anzen.json")
    if [[ $old_address == "$new_address" ]]; then
        printf 'ERROR: emergency key rotation reused the vault address\n' >&2
        exit 1
    fi
    confirm_transaction "Confirming emergency key rotation"
    require_live_controllers 2
    anzen "$MAIN" status
    if [[ $(jq -r '.monthly_limit_sats' "$MAIN/anzen.json") != "$DEFAULT_MONTHLY_LIMIT_SATS" ]]; then
        printf 'ERROR: phone-key rotation did not preserve the monthly limit\n' >&2
        exit 1
    fi
    if [[ $(jq -r '.emergency_access_limit_sats' "$MAIN/anzen.json") != 50000000 || \
        $(jq -r '.emergency_access.amount_sats' "$MAIN/phone/schedule.json") != 50000000 ]]; then
        printf 'ERROR: phone-key rotation did not preserve emergency access\n' >&2
        exit 1
    fi
    if [[ $(jq '.entries | length' "$MAIN/phone/schedule.json") -ne 12 ]]; then
        printf 'ERROR: phone-key rotation did not create a replacement annual schedule\n' >&2
        exit 1
    fi
    success "Phone restored, key rotated, and vault policy preserved."

    first_unlock=$((first_delay_base + MONTHLY_ALLOWANCE_DELAY_SECONDS))
    advance_mtp_to "$first_unlock" "Fast-forward through the replacement allowance delay"
    step "Use the monthly allowance preserved through rotation"
    anzen "$MAIN" phone authorize 1
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
    anzen "$attacker_dir" phone send "$attacker_address" 500000
    anzen "$attacker_dir" phone create-sweep "$attacker_address" \
        --output "${E2E_TEST}-attacker-sweep.json"
    expect_failure "the stolen phone lacks H for an immediate cold-vault sweep" \
        "$attacker_dir" hww confirm-sweep "${E2E_TEST}-attacker-sweep.json" \
        --output "${E2E_TEST}-attacker-approved-sweep.json" --yes
    printf 'The phone key has no path of its own to the vault.\n'
    success "Hot funds exposed; cold vault remained protected."

    step "The owner restores the phone key through the HWW and rotates immediately"
    show_file_command rm -- "$MAIN/phone/device.json"
    rm -- "$MAIN/phone/device.json"
    restore_phone "$MAIN"
    rotate_phone_key "$MAIN"
    confirm_transaction "Confirming emergency key rotation"
    anzen "$MAIN" status
    success "Owner restored and rotated away from stolen key."
}

test_lost_hww() {
    local delay_base unlock recovery_address
    setup_vault

    step "Presign the annual schedule, then lose the HWW (its maker's seed backup survives)"
    ceremony "$NOW"
    delay_base=$(node_mtp)
    confirm_transaction "Confirming the annual rollover"
    unlock=$((delay_base + MONTHLY_ALLOWANCE_DELAY_SECONDS))
    show_file_command cp -- "$MAIN/hww/device.json" "$DEMO_ROOT/hww-seed-backup.json"
    cp -- "$MAIN/hww/device.json" "$DEMO_ROOT/hww-seed-backup.json"
    show_file_command rm -- "$MAIN/hww/device.json"
    rm -- "$MAIN/hww/device.json"
    success "Annual schedule presigned before HWW loss."

    advance_mtp_to "$unlock" "Fast-forward through the first allowance delay"

    step "Use the phone-held allowance without the HWW"
    anzen "$MAIN" phone authorize 1
    confirm_transaction "Confirming the phone-held monthly authorization"
    anzen_capture recovery_address "$MAIN" phone receive-address
    recovery_address=$(printf '%s\n' "$recovery_address" | awk '/^Hot receive address:/ {print $4}')
    anzen "$MAIN" phone create-sweep "$recovery_address" --output "${E2E_TEST}-sweep.json"
    expect_failure "the phone alone cannot move the savings" \
        "$MAIN" hww confirm-sweep "${E2E_TEST}-sweep.json" \
        --output "${E2E_TEST}-approved-sweep.json" --yes
    success "Monthly spend worked; the savings stayed in the vault."

    step "Restore the HWW from its seed backup: the same key, so the vault is unchanged"
    show_file_command cp -- "$DEMO_ROOT/hww-seed-backup.json" "$MAIN/hww/device.json"
    cp -- "$DEMO_ROOT/hww-seed-backup.json" "$MAIN/hww/device.json"
    cooperative_sweep "$MAIN" "$recovery_address"
    confirm_transaction "Confirming the cooperative sweep with the restored HWW"
    status_compact
    success "Restored HWW and phone moved the vault together."
}

test_stolen_hww() {
    local attacker_dir="$DEMO_ROOT/stolen-hww-attacker"
    local fresh_dir="$DEMO_ROOT/stolen-hww-fresh-vault"
    local attacker_address fresh_vault
    setup_vault
    make_receiver attacker_address "Attacker"

    step "Simulate HWW theft: the attacker has H; the owner keeps M and the HWW's seed backup"
    show_file_command cp -a "$MAIN" "$attacker_dir"
    cp -a "$MAIN" "$attacker_dir"
    show_file_command rm -- "$attacker_dir/phone/device.json"
    rm -- "$attacker_dir/phone/device.json"
    expect_failure "the stolen HWW must wait out the ~388-day time lock" \
        "$attacker_dir" hww recover "$attacker_address"
    success "Stolen HWW blocked by the time lock."

    step "The owner retires the stolen key: a new vault with a fresh signer"
    command anzen --data-dir "$fresh_dir" phone init >/dev/null
    command anzen --data-dir "$fresh_dir" hww init >/dev/null
    command anzen --data-dir "$fresh_dir" init >/dev/null
    fresh_vault=$(jq -r .vault_address "$fresh_dir/anzen.json")
    printf 'Fresh vault: %s\n' "$fresh_vault"
    cooperative_sweep "$MAIN" "$fresh_vault"
    confirm_transaction "Confirming the move to the fresh vault"
    success "Savings moved before the stolen HWW's time lock opened."

    advance_mtp_to "$((FUNDING_START + HWW_RECOVERY_SECONDS))" \
        "Fast-forward past the stolen HWW's ~388-day time lock"
    expect_failure "the old vault is already empty" \
        "$attacker_dir" hww recover "$attacker_address"
    success "Stolen HWW reached its time lock too late."
}

test_lost_phone_no_cloud() {
    local recovery_address
    setup_vault
    make_receiver recovery_address "Recovered owner"

    step "Simulate losing the phone and its cloud backup"
    show_file_command rm -rf -- "$MAIN/phone" "$MAIN/anzen.json" "$MAIN/cloud/phone-seed-backup.json"
    rm -rf -- "$MAIN/phone" "$MAIN/anzen.json" "$MAIN/cloud/phone-seed-backup.json"
    anzen_filtered '
        /^(Simulated phone initialized|Phone vault key:)/ { print }
    ' "$MAIN" phone init
    anzen "$MAIN" hww export-recovery --output "${E2E_TEST}-recovery-bundle.json"
    anzen "$MAIN" phone import-recovery "${E2E_TEST}-recovery-bundle.json"
    expect_failure "there is no encrypted phone backup to restore" \
        "$MAIN" phone restore-request "$MAIN/cloud/phone-seed-backup.json" \
        --output "${E2E_TEST}-missing-restore.json"
    expect_failure "the HWW fallback is not mature yet" \
        "$MAIN" hww recover "$recovery_address"
    success "Missing backup confirmed; early HWW recovery blocked."

    advance_mtp_to "$((FUNDING_START + HWW_RECOVERY_SECONDS))" \
        "Fast-forward past the HWW's ~388-day time lock"
    step "Recover with the HWW alone"
    anzen "$MAIN" hww recover "$recovery_address"
    confirm_transaction "Confirming the HWW-only recovery sweep"
    status_compact
    success "Surviving HWW recovered the vault after its time lock."
}

test_both_lost() {
    local recovery_address
    setup_vault
    make_receiver recovery_address "Replacement owner"

    step "Simulate losing both devices and the HWW's seed backup; encrypted cloud data survives"
    show_file_command rm -rf -- "$MAIN/phone" "$MAIN/anzen.json" "$MAIN/hww/device.json"
    rm -rf -- "$MAIN/phone" "$MAIN/anzen.json" "$MAIN/hww/device.json"
    anzen_filtered '
        /^(Simulated phone initialized|Phone vault key:)/ { print }
    ' "$MAIN" phone init
    anzen "$MAIN" phone import-recovery "$MAIN/hww/vault-config.json"
    anzen "$MAIN" phone restore-request "$MAIN/cloud/phone-seed-backup.json" \
        --output "${E2E_TEST}-restore-request.json"
    expect_failure "the backup cannot be decrypted without the HWW" \
        "$MAIN" hww approve-restore "${E2E_TEST}-restore-request.json" \
        --output "${E2E_TEST}-restore-response.json" --yes
    success "Both device keys lost; backup cannot be decrypted."

    advance_mtp_to "$((FUNDING_START + HWW_RECOVERY_SECONDS))" \
        "Fast-forward past the HWW's ~388-day time lock"
    expect_failure "no surviving key can sign the open HWW path" \
        "$MAIN" hww recover "$recovery_address"
    printf 'Neither device nor the HWW seed backup survives, so no key exists.\n'
    success "Funds remain unrecoverable without a key."
}

test_cloud_compromise() {
    local attacker_dir="$DEMO_ROOT/cloud-attacker"
    local attacker_address
    setup_vault
    make_receiver attacker_address "Attacker"

    step "Simulate theft of only the encrypted cloud backup"
    show_file_command mkdir -p "$attacker_dir/cloud"
    mkdir -p "$attacker_dir/cloud"
    show_file_command cp "$MAIN/anzen.json" "$attacker_dir/anzen.json"
    cp "$MAIN/anzen.json" "$attacker_dir/anzen.json"
    show_file_command cp "$MAIN/cloud/phone-seed-backup.json" "$attacker_dir/cloud/phone-seed-backup.json"
    cp "$MAIN/cloud/phone-seed-backup.json" "$attacker_dir/cloud/phone-seed-backup.json"
    anzen_filtered '
        /^(Simulated phone initialized|Phone vault key:)/ { print }
    ' "$attacker_dir" phone init
    anzen "$attacker_dir" phone import-recovery "$attacker_dir/anzen.json"
    anzen "$attacker_dir" phone restore-request \
        "$attacker_dir/cloud/phone-seed-backup.json" \
        --output "${E2E_TEST}-cloud-restore-request.json"
    expect_failure "cloud ciphertext alone cannot restore the phone" \
        "$attacker_dir" hww approve-restore "${E2E_TEST}-cloud-restore-request.json" \
        --output "${E2E_TEST}-cloud-restore-response.json" --yes
    expect_failure "cloud ciphertext alone cannot sign a cooperative sweep" \
        "$attacker_dir" phone create-sweep "$attacker_address" \
        --output "${E2E_TEST}-cloud-sweep.json"
    success "Cloud ciphertext revealed no spending capability."
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
    local initial_start old_schedule anniversary current_mtp days
    setup_vault

    step "Create the first annual schedule"
    initial_start=$(node_mtp)
    ceremony "$NOW"
    confirm_transaction "Confirming the first annual rollover"
    require_live_controllers 1
    old_schedule="$DEMO_ROOT/rollover-on-time-old-schedule"
    cp -a "$MAIN" "$old_schedule"
    anzen "$MAIN" status
    success "Initial annual rollover confirmed with twelve allowance hops."

    anniversary=$((NOW + 365 * 24 * 60 * 60))
    advance_mtp_to "$anniversary" "Advance the calendar to the annual rollover date"
    anzen "$MAIN" status
    expect_failure "the HWW's time lock is still closed at the scheduled rollover" \
        "$MAIN" hww recover "$MINING_ADDRESS"
    success "Annual rollover date reached about three weeks before the HWW's time lock."

    step "Perform the scheduled cooperative rollover"
    current_mtp=$(node_mtp)
    ceremony "$current_mtp"
    confirm_transaction "Confirming the on-time annual rollover"
    require_live_controllers 1
    anzen "$MAIN" status
    days=$(hww_days_remaining)
    if (( days < 388 )); then
        printf 'ERROR: on-time rollover did not restart the HWW time lock (%s days left)\n' "$days" >&2
        exit 1
    fi
    success "All funds rolled over and the HWW's time lock restarted."

    step "Try a retained authorization from the previous annual schedule"
    expect_failure "the confirmed rollover spent the old allowance-chain output" \
        "$old_schedule" phone authorize 1
    success "Old allowance authorizations were invalidated by the rollover."

    advance_mtp_to "$((initial_start + HWW_RECOVERY_SECONDS))" \
        "Pass the previous schedule's HWW deadline"
    anzen "$MAIN" status
    expect_failure "the renewed outputs have a fresh time lock" \
        "$MAIN" hww recover "$MINING_ADDRESS"
    success "Old deadline passed while renewed funds stayed locked."
}

test_rollover_forgotten() {
    local current_mtp
    setup_vault

    step "Forget the annual rollover and inspect the original vault output"
    anzen "$MAIN" status
    advance_mtp_to "$((FUNDING_START + HWW_RECOVERY_SECONDS))" \
        "Fast-forward until the HWW-alone path opens"
    anzen "$MAIN" status
    if [[ $(hww_days_remaining) != 0 ]]; then
        printf 'ERROR: the HWW-alone path did not open\n' >&2
        exit 1
    fi
    success "Forgotten rollover opened only the HWW's path; the phone still has none."

    step "Perform a late cooperative rollover to restart the time lock"
    current_mtp=$(node_mtp)
    ceremony "$current_mtp"
    confirm_transaction "Confirming the late annual rollover"
    anzen "$MAIN" status
    if (( $(hww_days_remaining) < 388 )); then
        printf 'ERROR: the late rollover did not restart the HWW time lock\n' >&2
        exit 1
    fi
    success "Late rollover restarted the HWW's time lock."
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
    emergency-access) test_emergency_access ;;
    emergency-cancel) test_emergency_cancel ;;
    hww-revoke) test_hww_revoke ;;
    partial-funding) test_partial_funding ;;
    lost-phone) test_lost_phone ;;
    stolen-phone) test_stolen_phone ;;
    lost-hww) test_lost_hww ;;
    stolen-hww) test_stolen_hww ;;
    lost-phone-no-cloud) test_lost_phone_no_cloud ;;
    both-lost) test_both_lost ;;
    cloud-compromise) test_cloud_compromise ;;
    both-compromised) test_both_compromised ;;
    rollover-on-time) test_rollover_on_time ;;
    rollover-forgotten) test_rollover_forgotten ;;
esac

printf '\n✨ Test passed: %s\n' "$E2E_TEST"
