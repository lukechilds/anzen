# Vault CLI MVP

[![CI](https://github.com/lukechilds/vault/actions/workflows/ci.yml/badge.svg)](https://github.com/lukechilds/vault/actions/workflows/ci.yml)

This repository contains a regtest-only Rust/BDK implementation of the renewable Bitcoin vault described in [vault-design.md](vault-design.md). It uses a real Bitcoin Core node for policy validation, transaction relay, calendar locktimes, and block-based recovery.

## Use the CLI manually

Start Core, then define the `vault` shell helper. The helper itself has no output. The Compose volume preserves vault state and JSON handoff files between invocations:

```console
$ docker compose --profile manual up -d bitcoind
Network vault_default Creating
Network vault_default Created
Container vault-bitcoind-1 Creating
Container vault-bitcoind-1 Created
Container vault-bitcoind-1 Starting
Container vault-bitcoind-1 Started

$ vault() { COMPOSE_PROGRESS=quiet docker compose run --rm cli "$@"; }
```

Everything is hard-wired to regtest and 1 sat/vB. Mnemonics are intentionally printed by the simulated devices for demonstration; this is not production key handling. Every value below was captured from the real commands against disposable Bitcoin Core regtest chains; some sections come from independent runs, and a new run generates different mnemonics, keys, addresses, and transaction IDs. The examples assume the vault has confirmed regtest funds; `./scripts/run-e2e.sh monthly-spend` demonstrates funding 2 BTC from a freshly mined hot wallet.

### Create a vault

Initialize each simulated device separately, then combine their public keys into the static cold-storage policy:

```console
$ vault phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: aware pear frame napkin satisfy success stove velvet increase style answer chat trash bamboo all omit shield enforce antique brick talent equip else roast
Phone vault key: 1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80

$ vault hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: gasp cricket sword blast unfold like garlic syrup tree hover discover twin win gold crisp solar vote logic iron sting face retreat collect knife
HWW vault key: f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32
Phone backup encrypted for the HWW

$ vault init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80,f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32),{and_v(v:older(61200),pk(1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80)),and_v(v:older(65535),pk(f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32))}})#lwqlwu4c
Vault address: bcrt1p0j6cwkqng7y7weum5sqln5573deqvu9ycxxf92k98mvzmd0k3zzq4skpuc
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ vault policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80,f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32),{and_v(v:older(61200),pk(1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80)),and_v(v:older(65535),pk(f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32))}})#lwqlwu4c
Vault address: bcrt1p0j6cwkqng7y7weum5sqln5573deqvu9ycxxf92k98mvzmd0k3zzq4skpuc
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
```

The new vault starts with monthly spending disabled. `vault init` prints the static cold-storage descriptor, vault address, and recovery delays, but does not create or sign a monthly spending policy.

### Set or replace the monthly policy

The phone proposes the policy and signs its side of every PSBT. The HWW independently validates the high-level policy, asks for one approval, and signs the complete batch. The phone verifies the approved JSON, broadcasts the rollover, and stores each authorization and revocation as an individually encrypted artifact:

```console
$ vault phone set-policy --monthly-limit 10000000 --output policy.json
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80,f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32),{and_v(v:older(61200),pk(1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80)),and_v(v:older(65535),pk(f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32))}})#lwqlwu4c
Vault address: bcrt1p0j6cwkqng7y7weum5sqln5573deqvu9ycxxf92k98mvzmd0k3zzq4skpuc
Monthly limit: 10000000 sats
Fee rate: 1 sat/vB
Total input: 200000000 sats
Monthly pairs: 12
Rollover txid: dc4f2864de3b4ca64449322b270c37528283fbd02303400c2387a106e067d93e
Rollover fee: 635 sats
Phone signed PSBTs: 25
Phone-signed policy proposal: policy.json

$ vault hww confirm-policy policy.json --output approved-policy.json
SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80,f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32),{and_v(v:older(61200),pk(1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80)),and_v(v:older(65535),pk(f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32))}})#lwqlwu4c
Vault address: bcrt1p0j6cwkqng7y7weum5sqln5573deqvu9ycxxf92k98mvzmd0k3zzq4skpuc
Monthly limit: 10000000 sats
Fee rate: 1 sat/vB
Total input: 200000000 sats
Monthly pairs: 12
Rollover txid: dc4f2864de3b4ca64449322b270c37528283fbd02303400c2387a106e067d93e
Rollover fee: 635 sats
Phone signed PSBTs: 25
Type `approve` to confirm the complete monthly policy: approve
HWW validated and signed all 25 PSBTs after one approval
HWW-approved policy: approved-policy.json

$ vault phone activate-policy approved-policy.json
Rollover broadcast: dc4f2864de3b4ca64449322b270c37528283fbd02303400c2387a106e067d93e
Active monthly limit: 10000000 sats
Encrypted monthly transaction pairs: 12

$ vault policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80,f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32),{and_v(v:older(61200),pk(1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80)),and_v(v:older(65535),pk(f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32))}})#lwqlwu4c
Vault address: bcrt1p0j6cwkqng7y7weum5sqln5573deqvu9ycxxf92k98mvzmd0k3zzq4skpuc
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly limit: 10000000 sats
Presigned monthly transaction pairs: 12
```

`10000000` sats is 0.1 BTC. Set `--monthly-limit 0` through the same three-step protocol to disable monthly authorizations while still rolling all funds into cold storage. Policy JSON may also be piped with `--output -`; file handoff is clearer for the interactive HWW approval.

### Execute a monthly spend

The month is the calendar month recorded in the active schedule. An authorization becomes valid once Bitcoin median-time-past is beyond 00:00 UTC on its first day:

```console
$ vault phone authorize 2026-09
Broadcast Authorization for 2026-09: 24ac943e4879ea63ec69716c072bf530a0fe669b8cccb910dd81c88ddc2fb682
```

To keep only a 0.01 BTC soft limit from a 0.1 BTC authorization, immediately return the difference to cold storage:

```console
$ vault phone apply-soft-limit 2026-09 --limit 1000000
Soft limit applied for 2026-09: retained at most 1000000 sats hot; cold-return txid=1f5dad2a163ad191ed413f94a147f70a7b3f76afcc2e5a5891674c500cb98a3d
```

The signed monthly limit is the security boundary. The adjustable soft limit is a phone-side action and may be any value from zero through the signed monthly limit.

### Revoke a future monthly spend

Before an authorization matures, the phone can broadcast its conflicting presigned revocation without the HWW:

```console
$ vault phone revoke 2026-10
Broadcast Revocation for 2026-10: 7bae75c8f1f87bb993d5e4553c8c851afb5b2ea25672806005b9714fc83565b6
```

Once the revocation confirms, the corresponding authorization can no longer spend that monthly chunk.

### Replace a lost phone

If the encrypted cloud backup survives, the HWW decrypts it into a portable recovery object. After installing that object on the replacement phone, rotate immediately to a fresh phone key and vault address:

```console
$ vault hww decrypt-phone-backup \
  .vault-data/cloud/phone-seed-backup.json \
  --output phone-recovery.json
Decrypted phone recovery package: phone-recovery.json

$ vault phone restore phone-recovery.json
Phone key restored from HWW recovery package
Recovered phone mnemonic: salt undo ice ten tray circle trophy escape wrong token unusual check harbor feature floor wasp secret achieve keen spice model above nephew mutual

$ vault phone rotate-key --output phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: 242a7a74e9cc57b1a35d32af276defe8390b970d8978f2595985ef30441ccaba
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,242a7a74e9cc57b1a35d32af276defe8390b970d8978f2595985ef30441ccaba,a513c47e38d07886f628d7e8e822f212ea64dcfe600c56e0abcc601113ff0a1b),{and_v(v:older(61200),pk(242a7a74e9cc57b1a35d32af276defe8390b970d8978f2595985ef30441ccaba)),and_v(v:older(65535),pk(a513c47e38d07886f628d7e8e822f212ea64dcfe600c56e0abcc601113ff0a1b))}})#gjtqear8
New vault address: bcrt1p7p2jazc7cz3j5m622qp8lh8yquygu8hq848rlmn7rgep3m2cs2jssmjk0j
Inputs: 12
Sent: 199998015 sats
Fee: 1350 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed monthly pairs: 12
Renewed policy PSBTs: 25
Phone-key rotation proposal: phone-rotation.json

$ vault hww confirm-rotation phone-rotation.json \
  --output approved-phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: 242a7a74e9cc57b1a35d32af276defe8390b970d8978f2595985ef30441ccaba
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,242a7a74e9cc57b1a35d32af276defe8390b970d8978f2595985ef30441ccaba,a513c47e38d07886f628d7e8e822f212ea64dcfe600c56e0abcc601113ff0a1b),{and_v(v:older(61200),pk(242a7a74e9cc57b1a35d32af276defe8390b970d8978f2595985ef30441ccaba)),and_v(v:older(65535),pk(a513c47e38d07886f628d7e8e822f212ea64dcfe600c56e0abcc601113ff0a1b))}})#gjtqear8
New vault address: bcrt1p7p2jazc7cz3j5m622qp8lh8yquygu8hq848rlmn7rgep3m2cs2jssmjk0j
Inputs: 12
Sent: 199998015 sats
Fee: 1350 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed monthly pairs: 12
Renewed policy PSBTs: 25
Type `approve` to confirm the phone-key rotation: approve
HWW validated and signed the phone-key rotation plus 25 renewed-policy PSBTs
HWW-approved phone-key rotation: approved-phone-rotation.json

$ vault phone activate-rotation approved-phone-rotation.json
Emergency phone-key rotation broadcast: 91103a661f8979e67750262708ebbd364fded0fc3d03477800ac9f2efc62828b
Old vault address: bcrt1p6gdq3v0ygy8d5590cwhqtaxxwt94qzqwum4tmh89c82mft0zprvqp0yt2r
New vault address: bcrt1p7p2jazc7cz3j5m622qp8lh8yquygu8hq848rlmn7rgep3m2cs2jssmjk0j
New phone mnemonic: artwork decline hope sheriff slush economy enjoy balance jacket enemy hidden snap grid rent curious axis find protect fluid wrong expand correct rhythm figure
Monthly policy preserved: 10000000 sats
Policy rollover broadcast: 9422c1ac20f7f2c9f1a75deafbf89298a3c4b39a33908c7df9d953f948ad0b57
Encrypted monthly transaction pairs: 12
```

The rotation preserves the HWW key and active monthly limit, creates a new phone seed and HWW-encrypted backup, and sweeps the old vault cooperatively. The same proposal chains a fresh 12-month rollover to that sweep; one HWW prompt approves both, and the replacement monthly artifacts are encrypted to the new phone key. A vault whose monthly policy was disabled remains disabled after rotation.

If the phone and its backup are permanently unavailable, initialize a replacement vault, wait the real 65,535-block HWW delay, and recover directly to its address:

```console
$ vault --data-dir .replacement-vault phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: inform skate door head purity crouch supreme veteran season depart trophy west jelly rain excess legend manage source brother immense drop enough choose behave
Phone vault key: 80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a

$ vault --data-dir .replacement-vault hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: awful elephant tray grant fitness purity lock slam sauce segment company brain off aware lawn reward mercy middle method fee cheap wrestle another erase
HWW vault key: b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c
Phone backup encrypted for the HWW

$ vault --data-dir .replacement-vault init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ vault --data-dir .replacement-vault policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ vault hww recover bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
HWW recovery sweep broadcast: 157bdcb335fea687c1896042d5d4ee304b41933f1f140a1d55835b2ed82379d2
Inputs: 1
Sent: 4999999854 sats
Fee: 146 sats (1 sat/vB)
```

This delayed recovery moves the funds to a new phone key, a new HWW key, and a new static vault address.

### Replace a lost HWW

The phone can continue using existing monthly artifacts while the recovery delay runs. Initialize a replacement vault, wait the real 61,200-block phone delay, then sweep the old vault into its address:

```console
$ vault --data-dir .replacement-vault phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: inform skate door head purity crouch supreme veteran season depart trophy west jelly rain excess legend manage source brother immense drop enough choose behave
Phone vault key: 80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a

$ vault --data-dir .replacement-vault hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: awful elephant tray grant fitness purity lock slam sauce segment company brain off aware lawn reward mercy middle method fee cheap wrestle another erase
HWW vault key: b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c
Phone backup encrypted for the HWW

$ vault --data-dir .replacement-vault init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ vault --data-dir .replacement-vault policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ vault phone recover bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery sweep broadcast: f9ee29feea85f607042a9338ea38e1eb4ae2854751d5b1f9c3823653fb58c340
Inputs: 1
Sent: 4999999854 sats
Fee: 146 sats (1 sat/vB)
```

The replacement vault has a fresh HWW key (and a fresh phone epoch), so the missing HWW can no longer participate. Approve a new monthly policy after the recovery confirms.

### Other cooperative sweeps

Arbitrary immediate vault sweeps retain the same explicit device boundary:

```console
$ vault phone create-sweep bcrt1pdd5tyx8967m3xyqkjzdr0dd9dvmkpa9lauldk3cxmkvjgyulle2qnfk68w --output sweep.json
COOPERATIVE VAULT SWEEP
Destination: bcrt1pdd5tyx8967m3xyqkjzdr0dd9dvmkpa9lauldk3cxmkvjgyulle2qnfk68w
Inputs: 1
Sent: 198997378 sats
Fee: 162 sats (1 sat/vB)
Phone signed: true
Phone-signed cooperative sweep: sweep.json

$ vault hww confirm-sweep sweep.json --output approved-sweep.json
COOPERATIVE VAULT SWEEP
Destination: bcrt1pdd5tyx8967m3xyqkjzdr0dd9dvmkpa9lauldk3cxmkvjgyulle2qnfk68w
Inputs: 1
Sent: 198997378 sats
Fee: 162 sats (1 sat/vB)
Phone signed: true
Type `approve` to confirm the cooperative sweep: approve
HWW validated and signed the cooperative sweep
HWW-approved cooperative sweep: approved-sweep.json

$ vault phone broadcast-sweep approved-sweep.json
Cooperative vault sweep broadcast: 960a6390f3e655d6c3480d0903eb8244ca814af16d8de073b7178bfcaa2b0852
Inputs: 1
Sent: 198997378 sats
Fee: 162 sats (1 sat/vB)
```

## Run the end-to-end tests

Docker is the only host dependency:

```bash
./scripts/run-e2e.sh --list
./scripts/run-e2e.sh monthly-spend
./scripts/run-e2e.sh lost-phone lost-hww
./scripts/run-e2e.sh all
```

With no arguments, the runner behaves like `all`. Selected tests run serially, and each gets a fresh regtest chain and vault state so it can be read and reproduced independently. Separate runner invocations also use isolated Compose projects, so concurrent local tests cannot stop or erase one another. Output is limited to user actions, the corresponding CLI commands, essential policy/transaction results, expected safety rejections, and compact mining progress. Displayed commands omit the internal `--data-dir` argument, retain the terminal's default color, and show their results in muted grey. Every completed step starts a new paragraph with a short `✅` outcome so the test can be understood by skimming those lines.

The named tests cover setup/policy, monthly spend, monthly revoke, partial funding, lost or stolen phone, lost or stolen HWW, missing cloud backup, both devices lost, cloud compromise, both keys compromised, and both on-time and forgotten annual rollover. The spend demonstrations fund exactly 2 BTC and build twelve 0.1 BTC allowances.

Recovery tests mine the real 61,200/65,535-block CSV delays, and the on-time rollover test mines a 52,560-block year before continuing to the old recovery deadline. Running one is intentionally slow; running `all` is substantially slower because every long-delay test proves its behavior on an independent chain.

## Run all tests

```bash
./scripts/run-tests.sh
```

This runs unit and CLI tests, focused real-node integration tests, and the slow recovery integration test. For fast local development without Docker:

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

## Continuous integration

GitHub Actions runs formatting, Clippy, unit/CLI tests, the real Bitcoin Core integration suite, and every isolated end-to-end test on pushes to `main` and on every pull request. The workflow can also be started manually. End-to-end tests run serially through `./scripts/run-e2e.sh all`; superseded runs on the same branch are cancelled because the exact recovery-delay tests take a while.

The quality job restores Cargo registry and `target/` data with the GitHub Actions cache. Docker jobs build through Buildx with separate GHA-backed `test` and `runtime` cache scopes, load the resulting images into the runner, and tell the Compose scripts to use those images without rebuilding. The Dockerfile compiles dependencies before copying application source, so dependency layers remain reusable when Rust code changes.
