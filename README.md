# Anzen

[![CI](https://github.com/lukechilds/anzen/actions/workflows/ci.yml/badge.svg)](https://github.com/lukechilds/anzen/actions/workflows/ci.yml)

Anzen is a Bitcoin wallet with the security of a 2-of-3 multisig, but the simplicity of a mobile hot wallet. It pairs a mobile hot wallet with a cold wallet that acts as a programmable vault, keeping savings in cold storage while making everyday access simple.

A core design philosophy is that the hardware wallet signs vault policies, not individual day-to-day transactions. In one approval ceremony, it authorizes the policy and presigns everything the hot wallet needs to execute that policy for the next year. The hardware wallet is then unnecessary until the policy changes or the vault timelocks are renewed on the same calendar date the following year.

A vault policy can permit the hot wallet to withdraw up to a predefined amount from cold storage each month. The hot wallet alone can execute an approved withdrawal or revoke it before it becomes available. This behavior is enforced by Bitcoin—not an Anzen server, custodian, or online co-signer—using Bitcoin Script, signatures, and timelocks.

Anzen is designed to be difficult to operate incorrectly. It guides the user through complete policy, renewal, revocation, and recovery operations instead of leaving them to construct transactions or work out the next step themselves.

Anzen is designed to make permanent loss extraordinarily difficult. It has no single point of failure: no single lost device or unavailable service can strand a correctly configured vault forever, and no single stolen key can immediately drain it. Every on-chain spending and recovery path is encoded in Bitcoin Script and enforced by Bitcoin’s consensus rules.

### What Anzen guarantees

- **No single-device failure:** either surviving key has a path to recover funds if the other key is permanently lost.
- **No immediate single-key theft:** one stolen key cannot spend the main vault before its delayed recovery path activates. An honest key holder can always rotate funds away from an attacker before the stolen key gets access.
- **Hardware-wallet independence:** routine monthly spending uses the phone alone. With annual rollover maintained, the hardware wallet normally needs to be accessed only once per year.
- **Programmable monthly liquidity:** the phone can withdraw a fixed, pre-approved amount from the vault each calendar month without another hardware-wallet prompt.
- **Phone-controlled revocation:** the phone can cancel a future monthly allowance by broadcasting its presigned revocation before the allowance becomes spendable.
- **Optional social recovery:** a configured recovery friend can decrypt the phone backup if both devices are lost.
- **Trustless enforcement:** the 2-of-2 spend and both delayed single-key recovery paths live entirely in Taproot. Presigned monthly transactions are enforced by ordinary Bitcoin signatures and locktimes.
- **No provider dependency:** Anzen does not rely on a company, server, or proprietary recovery service remaining available.

### Exact loss and theft behavior

| Scenario | What happens |
| --- | --- |
| Phone lost | The hardware wallet immediately decrypts the cloud backup of the phone key, allowing the phone key to be restored and rotated. |
| Hardware wallet lost | Existing monthly allowances continue to work from the phone. The phone-only recovery path activates after 61,200 blocks—about 14 months from confirmation, and potentially much sooner after the device is lost. |
| Phone stolen | The attacker may take the hot balance or matured allowances, but cannot immediately spend the main vault. The honest hardware-wallet holder can restore the backed-up phone key and rotate the vault before the delayed phone path activates. |
| Hardware wallet stolen | The attacker cannot immediately spend the vault. The honest phone’s recovery path activates first, leaving roughly a one-month priority window before the hardware-wallet-only path matures. |
| Both devices lost | If social recovery was configured, any approved recovery friend can decrypt the phone backup and use the delayed phone-recovery path to sweep into replacement keys. Without social recovery, permanent loss of both devices is unrecoverable. |
| Cloud backup stolen | The backup is encrypted. It is useless without the hardware wallet or a configured recovery friend’s private key. |
| Both signing keys stolen | The attacker can satisfy the immediate 2-of-2 path. No wallet can protect funds after every required signing key is compromised, so the vault must be rotated before that happens. |

The full protocol and its trade-offs are documented in [anzen-design.md](anzen-design.md).

## Use the CLI manually

Start the regtest node and define an `anzen` shell helper:

```console
$ docker compose --profile manual up -d bitcoind
Network anzen_default Creating
Network anzen_default Created
Container anzen-bitcoind-1 Creating
Container anzen-bitcoind-1 Created
Container anzen-bitcoind-1 Starting
Container anzen-bitcoind-1 Started

$ anzen() { COMPOSE_PROGRESS=quiet docker compose run --rm cli "$@"; }
```

These examples use disposable regtest wallets. The simulated devices print their mnemonics for demonstration.

### Create a vault

Initialize each simulated device separately, then combine their public keys into the static cold-storage policy:

For mainnet, pass `--dangerously-enable-mainnet` to every command. Select a chain backend with `--chain-backend rpc|electrum`.

```console
$ anzen phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: aware pear frame napkin satisfy success stove velvet increase style answer chat trash bamboo all omit shield enforce antique brick talent equip else roast
Phone vault key: 1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80

$ anzen hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: gasp cricket sword blast unfold like garlic syrup tree hover discover twin win gold crisp solar vote logic iron sting face retreat collect knife
HWW vault key: f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32
HWW ready to wrap the descriptor-bound cloud backup at anzen init

$ anzen init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80,f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32),{and_v(v:older(61200),pk(1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80)),and_v(v:older(65535),pk(f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32))}})#lwqlwu4c
Vault address: bcrt1p0j6cwkqng7y7weum5sqln5573deqvu9ycxxf92k98mvzmd0k3zzq4skpuc
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Cloud recovery backup: phone key + descriptor encrypted; 0 recovery friends

$ anzen policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80,f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32),{and_v(v:older(61200),pk(1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80)),and_v(v:older(65535),pk(f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32))}})#lwqlwu4c
Vault address: bcrt1p0j6cwkqng7y7weum5sqln5573deqvu9ycxxf92k98mvzmd0k3zzq4skpuc
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
```

The new vault starts with monthly spending disabled. `anzen init` prints the static cold-storage descriptor, vault address, and recovery delays, but does not create or sign a monthly spending policy.

### Set or replace the monthly policy

The phone proposes the policy and signs its side of every PSBT. The HWW independently validates the high-level policy, asks for one approval, and signs the complete batch. The rollover consolidates the vault into one output. A separately presigned split creates twelve exact monthly UTXOs plus one remainder only when the first authorization or revocation is attempted. The phone stores the split, each authorization, and each revocation as individually encrypted artifacts:

```console
$ anzen phone set-policy --monthly-limit 10000000 --output policy.json
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,01bc17c31d0931d7730d59553de0da23c2da57eda526a1537a13972460fa5e01,7cb8562a4fa4170dea7c3d1dba74694d3cb141a72ebdad05b47a439ddd7e76ab),{and_v(v:older(61200),pk(01bc17c31d0931d7730d59553de0da23c2da57eda526a1537a13972460fa5e01)),and_v(v:older(65535),pk(7cb8562a4fa4170dea7c3d1dba74694d3cb141a72ebdad05b47a439ddd7e76ab))}})#e9yszxkt
Vault address: bcrt1pe06tvdn32kn382e9f6yu8q2zdwks2mpy4hs8qhrcmhsr89ze8mgskeprwy
Monthly limit: 10000000 sats
Fee rate: 1 sat/vB
Total input: 200000000 sats
Monthly pairs: 12
Rollover txid: bdc9965ee5c49bb6fafa4094f7d19f247a200cbfea72377da70519c35a6079a9
Rollover fee: 162 sats
Deferred split txid: 2128661cf631086f3b0fabe4ac1faee6f4100c78ddb5f194fd24c89b0a0cb7fd
Deferred split fee: 678 sats
Exact monthly UTXO: 10000162 sats
Split remainder: 79997216 sats
Phone signed PSBTs: 26
Phone-signed policy proposal: policy.json

$ anzen hww confirm-policy policy.json --output approved-policy.json
SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,01bc17c31d0931d7730d59553de0da23c2da57eda526a1537a13972460fa5e01,7cb8562a4fa4170dea7c3d1dba74694d3cb141a72ebdad05b47a439ddd7e76ab),{and_v(v:older(61200),pk(01bc17c31d0931d7730d59553de0da23c2da57eda526a1537a13972460fa5e01)),and_v(v:older(65535),pk(7cb8562a4fa4170dea7c3d1dba74694d3cb141a72ebdad05b47a439ddd7e76ab))}})#e9yszxkt
Vault address: bcrt1pe06tvdn32kn382e9f6yu8q2zdwks2mpy4hs8qhrcmhsr89ze8mgskeprwy
Monthly limit: 10000000 sats
Fee rate: 1 sat/vB
Total input: 200000000 sats
Monthly pairs: 12
Rollover txid: bdc9965ee5c49bb6fafa4094f7d19f247a200cbfea72377da70519c35a6079a9
Rollover fee: 162 sats
Deferred split txid: 2128661cf631086f3b0fabe4ac1faee6f4100c78ddb5f194fd24c89b0a0cb7fd
Deferred split fee: 678 sats
Exact monthly UTXO: 10000162 sats
Split remainder: 79997216 sats
Phone signed PSBTs: 26
Type `approve` to confirm the complete monthly policy: approve
HWW validated and signed all 26 PSBTs after one approval
HWW-approved policy: approved-policy.json

$ anzen phone activate-policy approved-policy.json
Rollover broadcast: bdc9965ee5c49bb6fafa4094f7d19f247a200cbfea72377da70519c35a6079a9
Deferred monthly split encrypted: 2128661cf631086f3b0fabe4ac1faee6f4100c78ddb5f194fd24c89b0a0cb7fd
Active monthly limit: 10000000 sats
Encrypted monthly transaction pairs: 12

$ anzen policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,01bc17c31d0931d7730d59553de0da23c2da57eda526a1537a13972460fa5e01,7cb8562a4fa4170dea7c3d1dba74694d3cb141a72ebdad05b47a439ddd7e76ab),{and_v(v:older(61200),pk(01bc17c31d0931d7730d59553de0da23c2da57eda526a1537a13972460fa5e01)),and_v(v:older(65535),pk(7cb8562a4fa4170dea7c3d1dba74694d3cb141a72ebdad05b47a439ddd7e76ab))}})#e9yszxkt
Vault address: bcrt1pe06tvdn32kn382e9f6yu8q2zdwks2mpy4hs8qhrcmhsr89ze8mgskeprwy
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly limit: 10000000 sats
Presigned monthly transaction pairs: 12
```

`10000000` sats is 0.1 BTC. At the fixed MVP fee rate shown above, each monthly UTXO is exactly `10000000 + 162` sats. Set `--monthly-limit 0` through the same three-step protocol to disable monthly authorizations while still rolling all funds into one cold output. Policy JSON may also be piped with `--output -`; file handoff is clearer for the interactive HWW approval.

### Execute a monthly spend

The month is the calendar month recorded in the active schedule. An authorization becomes valid once Bitcoin median-time-past is beyond 00:00 UTC on its first day:

```console
$ anzen phone authorize 2026-09
Broadcast Authorization for 2026-09: 24ac943e4879ea63ec69716c072bf530a0fe669b8cccb910dd81c88ddc2fb682
```

On the first successful monthly action, the command also prints `Deferred monthly split broadcast: TXID` before the authorization or revocation. Later actions reuse that confirmed split without noisy duplicate output.

To keep only a 0.01 BTC soft limit from a 0.1 BTC authorization, immediately return the difference to cold storage:

```console
$ anzen phone apply-soft-limit 2026-09 --limit 1000000
Soft limit applied for 2026-09: retained at most 1000000 sats hot; cold-return txid=1f5dad2a163ad191ed413f94a147f70a7b3f76afcc2e5a5891674c500cb98a3d
```

The signed monthly limit is the security boundary. The adjustable soft limit is a phone-side action and may be any value from zero through the signed monthly limit.

### Revoke a future monthly spend

Before an authorization matures, the phone can broadcast its conflicting presigned revocation without the HWW:

```console
$ anzen phone revoke 2026-10
Deferred monthly split broadcast: 11b7434a56ece6744c8dcf2e0ef4cfdf5daad7d96229d6afc959c2ba17b503ce
Broadcast Revocation for 2026-10: acc9f3ff0f3ef4316c82e61379f64c07dd0370cb326019d08f1711e509b5fd9f
```

Once the revocation confirms, the corresponding authorization can no longer spend that monthly chunk.

### Replace a lost phone

If the encrypted cloud backup survives, the HWW decrypts it into a portable recovery object. After installing that object on the replacement phone, rotate immediately to a fresh phone key and vault address:

```console
$ anzen hww decrypt-phone-backup \
  .anzen-data/cloud/phone-seed-backup.json \
  --output phone-recovery.json
Decrypted phone recovery package: phone-recovery.json

$ anzen phone restore phone-recovery.json
Phone key restored from authenticated recovery package
Recovered phone mnemonic: sausage bomb path long need gossip between damp upper oil together verb window sign hamster funny select antenna dress curtain pond motor sight female

$ anzen phone rotate-key --output phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: e92a75a14c841e76b3aa309056e6d64a69ee00c688ae7713d41b67b3abdf37fe
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,e92a75a14c841e76b3aa309056e6d64a69ee00c688ae7713d41b67b3abdf37fe,d1ee9e02a8a74c86b139534e285e4668f92a37ebdb18f6d7f2deec142b9e4f72),{and_v(v:older(61200),pk(e92a75a14c841e76b3aa309056e6d64a69ee00c688ae7713d41b67b3abdf37fe)),and_v(v:older(65535),pk(d1ee9e02a8a74c86b139534e285e4668f92a37ebdb18f6d7f2deec142b9e4f72))}})#dmum97ly
New vault address: bcrt1p5emqettxu3dx0cyflusv7f8um4gt9hldyx8u7m737ap2p2uhmpzqqfmy8l
Inputs: 1
Sent: 199999676 sats
Fee: 162 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed monthly pairs: 12
Renewed policy PSBTs: 26
Phone-key rotation proposal: phone-rotation.json

$ anzen hww confirm-rotation phone-rotation.json \
  --output approved-phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: e92a75a14c841e76b3aa309056e6d64a69ee00c688ae7713d41b67b3abdf37fe
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,e92a75a14c841e76b3aa309056e6d64a69ee00c688ae7713d41b67b3abdf37fe,d1ee9e02a8a74c86b139534e285e4668f92a37ebdb18f6d7f2deec142b9e4f72),{and_v(v:older(61200),pk(e92a75a14c841e76b3aa309056e6d64a69ee00c688ae7713d41b67b3abdf37fe)),and_v(v:older(65535),pk(d1ee9e02a8a74c86b139534e285e4668f92a37ebdb18f6d7f2deec142b9e4f72))}})#dmum97ly
New vault address: bcrt1p5emqettxu3dx0cyflusv7f8um4gt9hldyx8u7m737ap2p2uhmpzqqfmy8l
Inputs: 1
Sent: 199999676 sats
Fee: 162 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed monthly pairs: 12
Renewed policy PSBTs: 26
Type `approve` to confirm the phone-key rotation: approve
HWW validated and signed the phone-key rotation plus 26 renewed-policy PSBTs
HWW-approved phone-key rotation: approved-phone-rotation.json

$ anzen phone activate-rotation approved-phone-rotation.json
Emergency phone-key rotation broadcast: 58b46cc66da89a75ea33f5f5bb4b6bbdf3db60d6a21d9bd2df8eb5809ee9b0fe
Old vault address: bcrt1pxgwz2g4k0cv5kjw3gdys6awzhpjjxxavry72q3v2887fsu9rylgsasw6h0
New vault address: bcrt1p5emqettxu3dx0cyflusv7f8um4gt9hldyx8u7m737ap2p2uhmpzqqfmy8l
New phone mnemonic: trip predict leaf wing word night soup snake code bubble multiply river antique brief buddy clap paper mind session captain join true vote believe
Monthly policy preserved: 10000000 sats
Policy rollover broadcast: 6dfc083670f67cdde7e08804d6019dd349470c4130d68905550dc8a0a6fd2c36
Encrypted monthly transaction pairs: 12
```

The rotation preserves the HWW key, every configured recovery friend, and the active monthly limit. It creates a new phone seed and descriptor-bound cloud envelope, then sweeps the old vault cooperatively. The same proposal chains a fresh 12-month rollover and deferred split to that sweep; one HWW prompt approves both, and the replacement monthly artifacts are encrypted to the new phone key. A vault whose monthly policy was disabled remains disabled after rotation.

### Configure social recovery and emergency access

The backup payload contains the phone mnemonic and cold-storage descriptor, authenticated-encrypted under one random symmetric key. The HWW holds one encrypted copy of that key. Each recovery friend can hold another copy encrypted to their OpenPGP public key; the complete friend list is also authenticated by the symmetric key so cloud tampering cannot silently change who survives a later rotation. Friends are independent 1-of-N recipients, not threshold shares.

The key generator is a simulation convenience. In a real integration, import a public key whose private half stays under the friend's control:

```console
$ anzen social generate-friend-key --name "Alice <alice@example.test>" --public-key alice.pub.asc --private-key alice.sec.asc
Recovery friend OpenPGP key generated: 6e322d52f896b054dba2bb8dda013805966ab3b9
Public key: alice.pub.asc
Private key: alice.sec.asc (give only to the recovery friend)

$ anzen hww add-recovery-friend alice.pub.asc --yes
SIMULATED HWW — ADD RECOVERY FRIEND
OpenPGP fingerprint: 6e322d52f896b054dba2bb8dda013805966ab3b9
This friend gains the phone key and descriptor if they obtain the cloud backup
The 61,200-block phone recovery delay still applies to vault funds
Recovery friend added: 6e322d52f896b054dba2bb8dda013805966ab3b9
Cloud backup now grants this friend delayed phone recovery access
```

If both devices are lost, the friend can authenticate and decrypt the portable recovery package. The command displays the recovered public binding but writes the mnemonic only inside the private JSON output:

```console
$ anzen social decrypt-backup .anzen-data/cloud/phone-seed-backup.json --private-key alice.sec.asc --output friend-recovery.json
Social recovery decrypted and authenticated
Phone vault key: 45ef4f2557cc8efd84be6ce759be2c21ba1544414abd21e2faaa9adb89334461
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,45ef4f2557cc8efd84be6ce759be2c21ba1544414abd21e2faaa9adb89334461,63d061b6174bc25b0b8d59b67c2cfa5007047dd881d81d5e066aef41244aeb46),{and_v(v:older(61200),pk(45ef4f2557cc8efd84be6ce759be2c21ba1544414abd21e2faaa9adb89334461)),and_v(v:older(65535),pk(63d061b6174bc25b0b8d59b67c2cfa5007047dd881d81d5e066aef41244aeb46))}})#kl506xt5
Vault address: bcrt1p4cxhd740serrj3mj6uthpsrthagr5q2h2exmqzgzsamwpc280gzqs9x74x
Friend-decrypted phone recovery package: friend-recovery.json
```

Social recovery reconstructs `M`, not `H`, so it cannot bypass the vault script. After the real 61,200-block phone delay matures, the friend can sweep directly to replacement keys without installing either lost device:

```console
$ anzen social emergency-access .anzen-data/cloud/phone-seed-backup.json --private-key alice.sec.asc bcrt1p66y0chds0sua7yj22egwnm75hzzj4c5xpyqv4lqe4wtp8ffknpns3pmxl6
Social emergency-access sweep broadcast: bf4a1423cd2ce3e79647662ab638bcf6fa0285946916ad9787abc9bc0bc6a0b6
Inputs: 1
Sent: 199999854 sats
Fee: 146 sats (1 sat/vB)
On-chain phone recovery delay was enforced
```

Possession of a configured friend's private key is eventual phone-key capability. The HWW must therefore show that trust expansion clearly before adding a friend. The MVP-generated private key is unencrypted on disk; production friend-key UX and threshold recovery remain future work.

If the phone and its backup are permanently unavailable, initialize a replacement vault, wait the real 65,535-block HWW delay, and recover directly to its address:

```console
$ anzen --data-dir .replacement-vault phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: inform skate door head purity crouch supreme veteran season depart trophy west jelly rain excess legend manage source brother immense drop enough choose behave
Phone vault key: 80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a

$ anzen --data-dir .replacement-vault hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: awful elephant tray grant fitness purity lock slam sauce segment company brain off aware lawn reward mercy middle method fee cheap wrestle another erase
HWW vault key: b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c
HWW ready to wrap the descriptor-bound cloud backup at anzen init

$ anzen --data-dir .replacement-vault init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Cloud recovery backup: phone key + descriptor encrypted; 0 recovery friends

$ anzen --data-dir .replacement-vault policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ anzen hww recover bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
HWW recovery sweep broadcast: 157bdcb335fea687c1896042d5d4ee304b41933f1f140a1d55835b2ed82379d2
Inputs: 1
Sent: 4999999854 sats
Fee: 146 sats (1 sat/vB)
```

This delayed recovery moves the funds to a new phone key, a new HWW key, and a new static vault address.

### Replace a lost HWW

The phone can continue using existing monthly artifacts while the recovery delay runs. Initialize a replacement vault, wait the real 61,200-block phone delay, then sweep the old vault into its address:

```console
$ anzen --data-dir .replacement-vault phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: inform skate door head purity crouch supreme veteran season depart trophy west jelly rain excess legend manage source brother immense drop enough choose behave
Phone vault key: 80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a

$ anzen --data-dir .replacement-vault hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: awful elephant tray grant fitness purity lock slam sauce segment company brain off aware lawn reward mercy middle method fee cheap wrestle another erase
HWW vault key: b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c
HWW ready to wrap the descriptor-bound cloud backup at anzen init

$ anzen --data-dir .replacement-vault init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Cloud recovery backup: phone key + descriptor encrypted; 0 recovery friends

$ anzen --data-dir .replacement-vault policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ anzen phone recover bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery sweep broadcast: f9ee29feea85f607042a9338ea38e1eb4ae2854751d5b1f9c3823653fb58c340
Inputs: 1
Sent: 4999999854 sats
Fee: 146 sats (1 sat/vB)
```

The replacement vault has a fresh HWW key (and a fresh phone epoch), so the missing HWW can no longer participate. Approve a new monthly policy after the recovery confirms.

### Other cooperative sweeps

Arbitrary immediate vault sweeps retain the same explicit device boundary:

```console
$ anzen phone create-sweep bcrt1pdd5tyx8967m3xyqkjzdr0dd9dvmkpa9lauldk3cxmkvjgyulle2qnfk68w --output sweep.json
COOPERATIVE VAULT SWEEP
Destination: bcrt1pdd5tyx8967m3xyqkjzdr0dd9dvmkpa9lauldk3cxmkvjgyulle2qnfk68w
Inputs: 1
Sent: 198997378 sats
Fee: 162 sats (1 sat/vB)
Phone signed: true
Phone-signed cooperative sweep: sweep.json

$ anzen hww confirm-sweep sweep.json --output approved-sweep.json
COOPERATIVE VAULT SWEEP
Destination: bcrt1pdd5tyx8967m3xyqkjzdr0dd9dvmkpa9lauldk3cxmkvjgyulle2qnfk68w
Inputs: 1
Sent: 198997378 sats
Fee: 162 sats (1 sat/vB)
Phone signed: true
Type `approve` to confirm the cooperative sweep: approve
HWW validated and signed the cooperative sweep
HWW-approved cooperative sweep: approved-sweep.json

$ anzen phone broadcast-sweep approved-sweep.json
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

The named tests cover setup/policy, monthly spend, monthly revoke, partial funding, lost or stolen phone, lost or stolen HWW, missing cloud backup, both devices lost, OpenPGP social recovery with delayed emergency access, cloud compromise, both keys compromised, and both on-time and forgotten annual rollover. The spend demonstrations fund exactly 2 BTC and build twelve exact 0.1 BTC-plus-fee monthly UTXOs behind a deferred split.

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

Implementation rationale, library boundaries, and CI details are recorded in [design-decisions.md](design-decisions.md).

## License

[MIT](LICENSE) © 2026 Luke Childs
