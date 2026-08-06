# Anzen

[![CI](https://github.com/lukechilds/anzen/actions/workflows/ci.yml/badge.svg)](https://github.com/lukechilds/anzen/actions/workflows/ci.yml)

Anzen is a Bitcoin wallet that achieves the security of a 2-of-3 multisig with the simplicity of a mobile hot wallet. It pairs a mobile hot wallet with a cold wallet that acts as a programmable vault, keeping savings in cold storage while making everyday access simple.

Anzen is an open protocol, and this repository is its reference wallet implementation. Any hardware wallet can implement the cold-wallet side to review and sign Anzen vault policies, while any mobile or desktop wallet can implement the hot-wallet side to propose and execute them. Compatible implementations can work together without depending on the Anzen reference app or any particular vendor.

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
- **Cancellable emergency liquidity:** once per annual vault epoch, the phone can start a larger pre-approved withdrawal, cancel it during a one-week safety window, or complete it after the delay.
- **Optional social recovery:** a configured recovery friend can decrypt the phone backup if both devices are lost.
- **Trustless enforcement:** the 2-of-2 spend and both delayed single-key recovery paths live entirely in Taproot. Presigned policy transactions are enforced by ordinary Bitcoin signatures and absolute/relative locktimes.
- **Open interoperability:** hardware-wallet, mobile-wallet, and desktop-wallet vendors can implement either side of the protocol without depending on Anzen software or infrastructure.
- **No provider dependency:** Anzen does not rely on a company, server, or proprietary recovery service remaining available.

### Exact loss and theft behavior

| Scenario | What happens |
| --- | --- |
| Phone lost | The hardware wallet immediately decrypts the cloud backup of the phone key, allowing the phone key to be restored and rotated. |
| Hardware wallet lost | Existing monthly allowances continue to work from the phone. The phone-only recovery path activates after 61,200 blocks (~14 months) from confirmation, and potentially much sooner after the device is lost. |
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
Emergency access: disabled
Cloud recovery backup: phone key + descriptor encrypted; 0 recovery friends

$ anzen policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80,f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32),{and_v(v:older(61200),pk(1fd91c3103d72ee97da949697d2b71d45e43f8ea4d2437466afaad1911c19f80)),and_v(v:older(65535),pk(f900571d8f6936e6c178d775406f78356c1492864078b0133233d7f05c98be32))}})#lwqlwu4c
Vault address: bcrt1p0j6cwkqng7y7weum5sqln5573deqvu9ycxxf92k98mvzmd0k3zzq4skpuc
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Emergency access: disabled
```

The new vault starts with monthly spending and emergency access disabled. `anzen init` prints the static cold-storage descriptor, vault address, and recovery delays, but does not create or sign a spending policy.

### Set or replace the vault policy

The phone proposes the policy and signs its side of every PSBT. The HWW independently validates the high-level policy, asks for one approval, and signs the complete batch. The rollover directly creates twelve exact monthly UTXOs plus one cold remainder. Each later monthly action spends its confirmed rollover output directly, while the phone stores every policy transaction as an individually encrypted artifact. This real regtest policy combines a 0.1 BTC monthly limit with one cancellable 0.5 BTC emergency withdrawal:

```console
$ anzen phone set-policy --monthly-limit 10000000 --emergency-access-limit 50000000 --output policy.json
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,9cb777b47a518d3a2236851999db5abd1ba7d3a7d5c0bdd255bf96e359e0e16a,5996bd6e147bb221fbb9800c072c7eb913ca3c667a6d45ecedb307fd5724a42b),{and_v(v:older(61200),pk(9cb777b47a518d3a2236851999db5abd1ba7d3a7d5c0bdd255bf96e359e0e16a)),and_v(v:older(65535),pk(5996bd6e147bb221fbb9800c072c7eb913ca3c667a6d45ecedb307fd5724a42b))}})#w9d92z6q
Vault address: bcrt1pdgkljly5u9wsy9tcyjv2h2jzzhm73qnhru6su29e5qrd323ggawsdqhmef
Monthly limit: 10000000 sats
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
Fee rate: 1 sat/vB
Total input: 200000000 sats
Monthly pairs: 12
Rollover txid: af53000b201698e52f6257134ef5096128a47375d8900adbb3aba6bce8be526a
Rollover fee: 678 sats
Exact monthly UTXO: 10000162 sats
Rollover remainder: 79997378 sats
Emergency trigger txid: 55e35e4c8292a779f04428f508b637a9dceedeaa2fffc97cf0db9e5193c42bca
Emergency withdrawal txid: 962b89621bf1c76e1c703d56434181997cbc3e5496941262d46970f2790ca6fc
Emergency cancellation txid: 01645ebe82423027e4c41fca6cffd74eb4e9bda4c3a0149d370cc75c9fb100a2
Emergency hot address: bcrt1pgrasjcwujj4ynldm8ctz7p4e0xcxmdfu2cst2nad8sj6vvyluthqegrx5z
Phone signed PSBTs: 28
Phone-signed policy proposal: policy.json

$ anzen hww confirm-policy policy.json --output approved-policy.json
SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,9cb777b47a518d3a2236851999db5abd1ba7d3a7d5c0bdd255bf96e359e0e16a,5996bd6e147bb221fbb9800c072c7eb913ca3c667a6d45ecedb307fd5724a42b),{and_v(v:older(61200),pk(9cb777b47a518d3a2236851999db5abd1ba7d3a7d5c0bdd255bf96e359e0e16a)),and_v(v:older(65535),pk(5996bd6e147bb221fbb9800c072c7eb913ca3c667a6d45ecedb307fd5724a42b))}})#w9d92z6q
Vault address: bcrt1pdgkljly5u9wsy9tcyjv2h2jzzhm73qnhru6su29e5qrd323ggawsdqhmef
Monthly limit: 10000000 sats
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
Fee rate: 1 sat/vB
Total input: 200000000 sats
Monthly pairs: 12
Rollover txid: af53000b201698e52f6257134ef5096128a47375d8900adbb3aba6bce8be526a
Rollover fee: 678 sats
Exact monthly UTXO: 10000162 sats
Rollover remainder: 79997378 sats
Emergency trigger txid: 55e35e4c8292a779f04428f508b637a9dceedeaa2fffc97cf0db9e5193c42bca
Emergency withdrawal txid: 962b89621bf1c76e1c703d56434181997cbc3e5496941262d46970f2790ca6fc
Emergency cancellation txid: 01645ebe82423027e4c41fca6cffd74eb4e9bda4c3a0149d370cc75c9fb100a2
Emergency hot address: bcrt1pgrasjcwujj4ynldm8ctz7p4e0xcxmdfu2cst2nad8sj6vvyluthqegrx5z
Phone signed PSBTs: 28
Type `approve` to confirm the complete vault policy: approve
HWW validated and signed all 28 PSBTs after one approval
HWW-approved policy: approved-policy.json

$ anzen phone activate-policy approved-policy.json
Rollover broadcast: af53000b201698e52f6257134ef5096128a47375d8900adbb3aba6bce8be526a
Active monthly limit: 10000000 sats
Encrypted monthly transaction pairs: 12
Active emergency access: 50000000 sats
Encrypted emergency transaction set: trigger, withdrawal, cancellation

$ anzen policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,9cb777b47a518d3a2236851999db5abd1ba7d3a7d5c0bdd255bf96e359e0e16a,5996bd6e147bb221fbb9800c072c7eb913ca3c667a6d45ecedb307fd5724a42b),{and_v(v:older(61200),pk(9cb777b47a518d3a2236851999db5abd1ba7d3a7d5c0bdd255bf96e359e0e16a)),and_v(v:older(65535),pk(5996bd6e147bb221fbb9800c072c7eb913ca3c667a6d45ecedb307fd5724a42b))}})#w9d92z6q
Vault address: bcrt1pdgkljly5u9wsy9tcyjv2h2jzzhm73qnhru6su29e5qrd323ggawsdqhmef
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly limit: 10000000 sats
Presigned monthly transaction pairs: 12
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
```

`10000000` sats is 0.1 BTC and `50000000` sats is 0.5 BTC. At the fixed MVP fee rate shown above, each monthly UTXO is exactly `10000000 + 162` sats. Set either limit to zero through the same three-step protocol to disable that feature. Policy JSON may also be piped with `--output -`; file handoff is clearer for the interactive HWW approval.

### Execute a monthly spend

The month is the calendar month recorded in the active schedule. An authorization becomes valid once Bitcoin median-time-past is beyond 00:00 UTC on its first day:

```console
$ anzen phone authorize 2026-09
Broadcast Authorization for 2026-09: a77384565ac05d2031cfa42d66877f2ad8af8500cf840daec7cf3315b8d152d9
```

The authorization spends its assigned confirmed rollover output directly, so it has no policy-parent transaction to publish first.

To keep only a 0.01 BTC soft limit from a 0.1 BTC authorization, immediately return the difference to cold storage:

```console
$ anzen phone apply-soft-limit 2026-09 --limit 1000000
Soft limit applied for 2026-09: retained at most 1000000 sats hot; cold-return txid=70e79928e910fede550d47049cf7a37782ef480ca3a6a85d5c82239c4b25f0c5
```

The signed monthly limit is the security boundary. The adjustable soft limit is a phone-side action and may be any value from zero through the signed monthly limit.

### Revoke a future monthly spend

Before an authorization matures, the phone can broadcast its conflicting presigned revocation without the HWW:

```console
$ anzen phone revoke 2026-10
Broadcast Revocation for 2026-10: e7877a42937e3d876e6548c500b22aa4102c9e5a65e0e613c01d5e1c508a24f3
```

Once the revocation confirms, the corresponding authorization can no longer spend that monthly chunk.

### Use or cancel emergency access

The policy authorizes one emergency trigger per vault epoch. Starting it spends the rollover remainder into a staging output plus cold change and begins the Bitcoin-enforced cancellation window:

```console
$ anzen phone emergency initiate
Emergency access initiated: 55e35e4c8292a779f04428f508b637a9dceedeaa2fffc97cf0db9e5193c42bca
Amount after delay: 50000000 sats
Cancellation window: 605184 seconds

$ anzen phone emergency withdraw
Error: failed to broadcast emergency access Withdrawal
```

After the trigger confirms and the one-week BIP68 delay elapses, the same command releases exactly the approved amount to the fresh hot-wallet address committed by the policy:

```console
$ anzen phone emergency withdraw
Emergency access withdrawal broadcast: 962b89621bf1c76e1c703d56434181997cbc3e5496941262d46970f2790ca6fc
```

Alternatively, the phone can cancel before maturity. These outputs are from the isolated cancellation test's own vault epoch:

```console
$ anzen phone emergency initiate
Emergency access initiated: a6aaf80a4a50cb2cfa5ac33cfedfed1fd2c61fb9e6c93352b346184be76bbb74
Amount after delay: 50000000 sats
Cancellation window: 605184 seconds

$ anzen phone emergency cancel
Emergency access cancelled: 6eb03fa72873f6cd3eb11f0a844e732f901e45466f8f9ee7ca7fc2e0959c651f

$ anzen phone emergency withdraw
Error: failed to broadcast emergency access Withdrawal
```

Once cancellation confirms, the delayed withdrawal remains invalid even after its timelock expires because the two transactions spend the same staging output. BIP68 uses 512-second units, so the enforced minimum is 605,184 seconds—seven days plus 6 minutes 24 seconds.

### Replace a lost phone

If the encrypted cloud backup survives, the HWW decrypts it into a portable recovery object. After installing that object on the replacement phone, rotate immediately to a fresh phone key and vault address:

```console
$ anzen hww decrypt-phone-backup \
  .anzen-data/cloud/phone-seed-backup.json \
  --output phone-recovery.json
Decrypted phone recovery package: phone-recovery.json

$ anzen phone restore phone-recovery.json
Phone key restored from authenticated recovery package
Recovered phone mnemonic: surge inflict wasp egg input chase regret reduce thank air loud satoshi frame train thank crack surge accuse hawk shop base shrug live frown

$ anzen phone rotate-key --output phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: a821d63f9940a1d2fd5f08715a0606ddfdb97ddee1dedda7454fac685536f3ba
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,a821d63f9940a1d2fd5f08715a0606ddfdb97ddee1dedda7454fac685536f3ba,5c25cdf433075a30bba13f4623aebcb71ed9b91363777c42ddb44c9de063f0f8),{and_v(v:older(61200),pk(a821d63f9940a1d2fd5f08715a0606ddfdb97ddee1dedda7454fac685536f3ba)),and_v(v:older(65535),pk(5c25cdf433075a30bba13f4623aebcb71ed9b91363777c42ddb44c9de063f0f8))}})#dh7wxv07
New vault address: bcrt1pr3vc2d6wjsryzf22nynlt8mh8kgqdej3qezslkpqrqmplhru9wlqzpue7t
Inputs: 13
Sent: 199997864 sats
Fee: 1458 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed monthly pairs: 12
Renewed policy PSBTs: 28
Emergency access preserved: 50000000 sats
Phone-key rotation proposal: phone-rotation.json

$ anzen hww confirm-rotation phone-rotation.json \
  --output approved-phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: a821d63f9940a1d2fd5f08715a0606ddfdb97ddee1dedda7454fac685536f3ba
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,a821d63f9940a1d2fd5f08715a0606ddfdb97ddee1dedda7454fac685536f3ba,5c25cdf433075a30bba13f4623aebcb71ed9b91363777c42ddb44c9de063f0f8),{and_v(v:older(61200),pk(a821d63f9940a1d2fd5f08715a0606ddfdb97ddee1dedda7454fac685536f3ba)),and_v(v:older(65535),pk(5c25cdf433075a30bba13f4623aebcb71ed9b91363777c42ddb44c9de063f0f8))}})#dh7wxv07
New vault address: bcrt1pr3vc2d6wjsryzf22nynlt8mh8kgqdej3qezslkpqrqmplhru9wlqzpue7t
Inputs: 13
Sent: 199997864 sats
Fee: 1458 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed monthly pairs: 12
Renewed policy PSBTs: 28
Emergency access preserved: 50000000 sats
Type `approve` to confirm the phone-key rotation: approve
HWW validated and signed the phone-key rotation plus 28 renewed-policy PSBTs
HWW-approved phone-key rotation: approved-phone-rotation.json

$ anzen phone activate-rotation approved-phone-rotation.json
Emergency phone-key rotation broadcast: 3d712fc23e473abfbced5b3de186223b2735f2280c500647d7cc61996814d19f
Old vault address: bcrt1px7vvkjgprh25ewcaujgh26z5phrch0vuyqgn9c29wv0pfmmzrylqwwvcvv
New vault address: bcrt1pr3vc2d6wjsryzf22nynlt8mh8kgqdej3qezslkpqrqmplhru9wlqzpue7t
New phone mnemonic: build indoor correct hint yellow tree ride long potato mercy bullet say february race exotic unfair term human purchase scare river lake bracket ball
Monthly policy preserved: 10000000 sats
Policy rollover broadcast: 03b4146482ff5a0f57a480292cb0fab82dc17d78c014db3f198e6728e1c78aed
Encrypted monthly transaction pairs: 12
Emergency access preserved: 50000000 sats
```

The rotation preserves the HWW key, every configured recovery friend, and the active monthly and emergency-access limits. It creates a new phone seed and descriptor-bound cloud envelope, then sweeps the old vault cooperatively. The same proposal chains a fresh annual policy to that sweep; one HWW prompt approves both, and all replacement artifacts are encrypted to the new phone key. A disabled feature remains disabled after rotation.

### Configure social recovery

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
Emergency access: disabled
Cloud recovery backup: phone key + descriptor encrypted; 0 recovery friends

$ anzen --data-dir .replacement-vault policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Emergency access: disabled

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
Emergency access: disabled
Cloud recovery backup: phone key + descriptor encrypted; 0 recovery friends

$ anzen --data-dir .replacement-vault policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a,b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c),{and_v(v:older(61200),pk(80156c4a68c7ffd16c68c10f1793e1fc0ca4c7c85453ddd8066f797f07a73a3a)),and_v(v:older(65535),pk(b15a0cac758482440d0a8c869ab4cd902e3c85d44902cf3806a372f09779650c))}})#ue605er0
Vault address: bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Emergency access: disabled

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

The named tests cover setup/policy, monthly spend, monthly revoke, successful and cancelled one-week emergency access, partial funding, lost or stolen phone, lost or stolen HWW, missing cloud backup, both devices lost, OpenPGP social recovery, cloud compromise, both keys compromised, and both on-time and forgotten annual rollover. The spend demonstrations fund exactly 2 BTC and create twelve exact 0.1 BTC-plus-fee monthly UTXOs directly in the annual rollover.

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
