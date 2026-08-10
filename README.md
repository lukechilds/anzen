# Anzen

[![CI](https://github.com/lukechilds/anzen/actions/workflows/ci.yml/badge.svg)](https://github.com/lukechilds/anzen/actions/workflows/ci.yml)

> [!WARNING]
> Anzen is an experimental reference implementation, not a production-ready wallet. The current hardware wallet is simulated in software, mnemonics are printed, and fees are fixed at 1 sat/vB. Use regtest to explore the protocol; do not secure significant mainnet funds with this prototype.

Anzen is a Bitcoin wallet that achieves the two-device compromise resistance of a 2-of-3 multivendor multisig with the simplicity of a mobile hot wallet. It combines a phone hot wallet for everyday use with an independent hardware wallet and a programmable Taproot vault for cold storage.

Most self-custody designs force a compromise: singlesig is simple but makes one key a catastrophic point of failure; multivendor multisig removes that failure but is difficult to operate; collaborative custody restores good UX by introducing a provider with a signing key. Anzen is designed to be secure, easy to use, and trustless at the same time. [Read the motivation behind the design.](https://lu.ke/self-custody-trilemma/)

**The experience of a hot wallet, backed by cold storage.** Think checking and savings: the phone is your checking account and the vault is your savings account. Once a year, the hardware wallet approves a human-readable policy covering monthly transfers, immediate revocations, and a larger emergency transfer with a one-week cancellation window. It validates and presigns the complete transaction graph in one ceremony; the phone stores it encrypted and can execute the policy for the rest of the year.

Immediate vault access requires both independent device keys. If either key is lost or compromised, the honest holder of the other key retains a Bitcoin-enforced route to safety. No company, server, custodian, or online cosigner can change those paths, and losing the presigned policy transactions cannot lose the underlying bitcoin.

Anzen is designed to be difficult to operate incorrectly. It guides complete policy, renewal, revocation, rotation, and recovery operations rather than exposing raw transaction machinery and leaving the user to assemble a safe procedure.

Anzen is an open protocol, and this repository is its reference wallet implementation. Any hardware wallet can implement the cold-wallet side, and any mobile or desktop wallet can implement the hot-wallet side. Compatible implementations can work together without the reference app or any particular vendor.

The [protocol design](anzen-design.md) specifies the exact wallet properties, transaction graph, assumptions, and loss or theft behavior. [Design decisions](design-decisions.md) explains why Anzen uses this model and records its engineering trade-offs.

## How the vault is constructed

Anzen's cold storage is a Taproot address controlled by two keys: the mobile key (`phone`) and the hardware-wallet key (`hww`). Every cold output uses the same vault script. Bitcoin accepts a spend through any one of these paths:

```text
phone + hww immediately
phone only after 14 months
hww only after 15 months
```

The delays belong to each individual UTXO and begin when that output confirms. They are not controlled by Anzen, a server, or a calendar. The earlier phone path gives an honest phone holder a priority window to rotate the vault if the HWW key is stolen.

### Miniscript policy

The policy in minimal Taproot Miniscript descriptor notation is:

```text
tr(
  NUMS,
  {
    multi_a(2,phone,hww),
    {
      and_v(v:older(61200),pk(phone)),
      and_v(v:older(65535),pk(hww))
    }
  }
)
```

This is Anzen's only vault script. All other behavior—including monthly allowances, revocations, emergency access, and annual renewal—is implemented with presigned transaction chains using Bitcoin-enforced relative timelocks.

### Annual vault layout and presigned transaction graph

Once per year, the phone proposes a policy and the HWW approves it once. Together they sign an annual rollover plus every transaction shown below. Only the rollover is broadcast immediately. After it confirms, the vault has one allowance-chain UTXO funding up to twelve sequential withdrawals and one remainder UTXO; all other transactions remain encrypted on the phone until needed.

![The sequential allowance chain and emergency transactions for an example 2.1 BTC annual vault policy](media/vault-utxo-layout.svg)

The solid outputs are confirmed on-chain after rollover; dashed outputs exist only if their presigned parent confirms. Each authorization/revocation pair spends the same live chain output, and each emergency withdrawal/cancellation pair spends the same staging output:

- **Authorize or revoke the chain:** after roughly 30 days, an authorization releases the fixed limit and creates the next smaller chain output, whose own delay starts when it confirms. The competing revocation is valid immediately and returns the entire remaining chain to the vault. If it confirms, the current authorization and every dependent later hop are permanently invalid.
- **Withdraw or cancel:** the emergency withdrawal becomes valid one week after the trigger confirms. Cancellation is valid immediately and returns the staged funds to the vault. If cancellation confirms first, the withdrawal is permanently invalid.

The phone can broadcast any of these approved actions without reconnecting the HWW. Presigned transactions are convenience permissions, not custody: losing them does not lose the bitcoin, because every unspent output still has the three vault-script paths above. A later annual rollover spends all remaining cold UTXOs, resets their recovery delays, and invalidates the previous epoch's unused presigned transactions.

For a concrete byte-level example of this graph—including txids, outpoints, locktimes, sequences, values, addresses, and scripts—see the checked-in [vault output test vector](test-vectors/vault-output-graph.json).

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

The phone proposes the policy and signs its side of every PSBT. The HWW independently validates the high-level policy, asks for one approval, and signs the complete batch. The rollover creates one allowance-chain UTXO plus one cold remainder. Every successful hop releases 0.1 BTC and creates the next smaller chain output after a fresh relative delay; every competing revocation cancels the entire remaining chain. The phone stores each transaction as an individually encrypted artifact. This real regtest policy combines a 0.1 BTC monthly limit with one cancellable 0.5 BTC emergency withdrawal:

```console
$ anzen phone set-policy --monthly-limit 10000000 --emergency-access-limit 50000000 --output policy.json
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,d7ee64997426f39e65e8a94d0ab51f8a9c166012014ec523a6faddeb0dd8ca1e,83676364fb1856e57822370ef6f4514487aa62297b8b28afd371b10fc8354be7),{and_v(v:older(61200),pk(d7ee64997426f39e65e8a94d0ab51f8a9c166012014ec523a6faddeb0dd8ca1e)),and_v(v:older(65535),pk(83676364fb1856e57822370ef6f4514487aa62297b8b28afd371b10fc8354be7))}})#apd43954
Vault address: bcrt1pjdns6u50wdgrn748e8jdvup4g3xn7qe43cf3myeq5fghn70hehzszljk6h
Monthly limit: 10000000 sats
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
Fee rate: 1 sat/vB
Total input: 200000000 sats
Allowance steps: 12
Allowance hop delay: 2592256 seconds (~30 days)
Rollover txid: 0c7ad7657257c4eb3e782921dd787dd806eba5a2e27d5fe96633859f24675b20
Rollover fee: 205 sats
Initial allowance-chain UTXO: 120002417 sats
Rollover remainder: 79997378 sats
Emergency trigger txid: 830bad271fa213eea0c92c84729f2ce3c0b1878e5317a4f74c9337d1771f9504
Emergency withdrawal txid: b99360d008858fccf8fa8028e747342b050bb3404d644e0db78ab55a867bb6c4
Emergency cancellation txid: c58d49702f06ace3b136f06e5a3fe1173582a687e42e689bdf6eb43d7d3b5eff
Emergency hot address: bcrt1plm54pf09x6ynzvgd25d0x9hxe54fquut50qmc8kr2a2rr746lfwss7g845
Phone signed PSBTs: 28
Phone-signed policy proposal: policy.json

$ anzen hww confirm-policy policy.json --output approved-policy.json
SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,d7ee64997426f39e65e8a94d0ab51f8a9c166012014ec523a6faddeb0dd8ca1e,83676364fb1856e57822370ef6f4514487aa62297b8b28afd371b10fc8354be7),{and_v(v:older(61200),pk(d7ee64997426f39e65e8a94d0ab51f8a9c166012014ec523a6faddeb0dd8ca1e)),and_v(v:older(65535),pk(83676364fb1856e57822370ef6f4514487aa62297b8b28afd371b10fc8354be7))}})#apd43954
Vault address: bcrt1pjdns6u50wdgrn748e8jdvup4g3xn7qe43cf3myeq5fghn70hehzszljk6h
Monthly limit: 10000000 sats
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
Fee rate: 1 sat/vB
Total input: 200000000 sats
Allowance steps: 12
Allowance hop delay: 2592256 seconds (~30 days)
Rollover txid: 0c7ad7657257c4eb3e782921dd787dd806eba5a2e27d5fe96633859f24675b20
Rollover fee: 205 sats
Initial allowance-chain UTXO: 120002417 sats
Rollover remainder: 79997378 sats
Emergency trigger txid: 830bad271fa213eea0c92c84729f2ce3c0b1878e5317a4f74c9337d1771f9504
Emergency withdrawal txid: b99360d008858fccf8fa8028e747342b050bb3404d644e0db78ab55a867bb6c4
Emergency cancellation txid: c58d49702f06ace3b136f06e5a3fe1173582a687e42e689bdf6eb43d7d3b5eff
Emergency hot address: bcrt1plm54pf09x6ynzvgd25d0x9hxe54fquut50qmc8kr2a2rr746lfwss7g845
Phone signed PSBTs: 28
Type `approve` to confirm the complete vault policy: approve
HWW validated and signed all 28 PSBTs after one approval
HWW-approved policy: approved-policy.json

$ anzen phone activate-policy approved-policy.json
Rollover broadcast: 0c7ad7657257c4eb3e782921dd787dd806eba5a2e27d5fe96633859f24675b20
Active monthly limit: 10000000 sats
Encrypted allowance transaction pairs: 12
Active emergency access: 50000000 sats
Encrypted emergency transaction set: trigger, withdrawal, cancellation

$ anzen policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,d7ee64997426f39e65e8a94d0ab51f8a9c166012014ec523a6faddeb0dd8ca1e,83676364fb1856e57822370ef6f4514487aa62297b8b28afd371b10fc8354be7),{and_v(v:older(61200),pk(d7ee64997426f39e65e8a94d0ab51f8a9c166012014ec523a6faddeb0dd8ca1e)),and_v(v:older(65535),pk(83676364fb1856e57822370ef6f4514487aa62297b8b28afd371b10fc8354be7))}})#apd43954
Vault address: bcrt1pjdns6u50wdgrn748e8jdvup4g3xn7qe43cf3myeq5fghn70hehzszljk6h
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly limit: 10000000 sats
Presigned allowance transaction pairs: 12
Allowance hop delay: 2592256 seconds (~30 days)
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
```

`10000000` sats is 0.1 BTC and `50000000` sats is 0.5 BTC. The initial chain output funds twelve 0.1 BTC releases plus every authorization fee. Set either limit to zero through the same three-step protocol to disable that feature. Policy JSON may also be piped with `--output -`; file handoff is clearer for the interactive HWW approval.

### Execute a monthly spend

Allowances are numbered sequentially. Step 1 becomes valid roughly 30 days after rollover confirmation; each later step becomes valid roughly 30 days after the preceding authorization confirms. The exact BIP68 delay is 2,592,256 seconds:

```console
$ anzen phone authorize 1
Broadcast Authorization for allowance step 1: 091329eb090e25b2bd2b90778e747d65c27065e15d02c6b5a479ff56a7b234b1
```

The authorization releases the approved amount at output 0 and creates step 2's chain output at output 1. Step 2 cannot mature before step 1 confirms, even if the phone waited much longer than 30 days before using step 1.

To keep only a 0.01 BTC soft limit from a 0.1 BTC authorization, immediately return the difference to cold storage:

```console
$ anzen phone apply-soft-limit 1 --limit 1000000
Soft limit applied for allowance step 1: retained at most 1000000 sats hot; cold-return txid=a5e76a5e9d294c45ac6f8e25ac420e0e5794a4b1720acf1576f9156fdb5fe200
```

The signed monthly limit is the security boundary. The adjustable soft limit is a phone-side action and may be any value from zero through the signed monthly limit.

### Revoke all remaining monthly spends

Once a hop's source output exists, the phone can broadcast its conflicting presigned revocation without the HWW:

```console
$ anzen phone revoke 2
Broadcast Revocation for allowance step 2: 5289189883b2f3a875af8684ada6b644e807df643eadd2dc815ed5162a893538
```

Once the revocation confirms, step 2 cannot spend the live chain output. Steps 3–12 are also invalid because they depend on transaction outputs that step 2 can no longer create. Revocation is therefore deliberately whole-chain rather than per-allowance.

### Use or cancel emergency access

The policy authorizes one emergency trigger per vault epoch. Starting it spends the rollover remainder into a staging output plus cold change and begins the Bitcoin-enforced cancellation window:

```console
$ anzen phone emergency initiate
Emergency access initiated: 830bad271fa213eea0c92c84729f2ce3c0b1878e5317a4f74c9337d1771f9504
Amount after delay: 50000000 sats
Cancellation window: 605184 seconds

$ anzen phone emergency withdraw
Error: failed to broadcast emergency access Withdrawal
```

After the trigger confirms and the one-week BIP68 delay elapses, the same command releases exactly the approved amount to the fresh hot-wallet address committed by the policy:

```console
$ anzen phone emergency withdraw
Emergency access withdrawal broadcast: b99360d008858fccf8fa8028e747342b050bb3404d644e0db78ab55a867bb6c4
```

Alternatively, the phone can cancel before maturity. These outputs are from the isolated cancellation test's own vault epoch:

```console
$ anzen phone emergency initiate
Emergency access initiated: 55df7b5e30f0cc52da85510798fa5f0b89c3edea00957589df41e045bc2ebcd7
Amount after delay: 50000000 sats
Cancellation window: 605184 seconds

$ anzen phone emergency cancel
Emergency access cancelled: a369bfbbfece58206e081f459574ef441e64f151359a96637467f656548d5b08

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
New phone vault key: f9c8ae08ff6e0a7f48d584f8bfee382901ea9c9d12baab5b1163372ccf740205
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,f9c8ae08ff6e0a7f48d584f8bfee382901ea9c9d12baab5b1163372ccf740205,18d4137264204041e2ac7bcc814d1b89b225269b8b2fb7b3d958bbf47142ff2b),{and_v(v:older(61200),pk(f9c8ae08ff6e0a7f48d584f8bfee382901ea9c9d12baab5b1163372ccf740205)),and_v(v:older(65535),pk(18d4137264204041e2ac7bcc814d1b89b225269b8b2fb7b3d958bbf47142ff2b))}})#4wfwncyz
New vault address: bcrt1pmdc3g35t8q4kmvj8vytcqswxwaz6djma8v9gl67upg4enkqwsmeq3qfmx3
Inputs: 2
Sent: 199999525 sats
Fee: 270 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed allowance steps: 12
Renewed policy PSBTs: 28
Emergency access preserved: 50000000 sats
Phone-key rotation proposal: phone-rotation.json

$ anzen hww confirm-rotation phone-rotation.json \
  --output approved-phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: f9c8ae08ff6e0a7f48d584f8bfee382901ea9c9d12baab5b1163372ccf740205
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,f9c8ae08ff6e0a7f48d584f8bfee382901ea9c9d12baab5b1163372ccf740205,18d4137264204041e2ac7bcc814d1b89b225269b8b2fb7b3d958bbf47142ff2b),{and_v(v:older(61200),pk(f9c8ae08ff6e0a7f48d584f8bfee382901ea9c9d12baab5b1163372ccf740205)),and_v(v:older(65535),pk(18d4137264204041e2ac7bcc814d1b89b225269b8b2fb7b3d958bbf47142ff2b))}})#4wfwncyz
New vault address: bcrt1pmdc3g35t8q4kmvj8vytcqswxwaz6djma8v9gl67upg4enkqwsmeq3qfmx3
Inputs: 2
Sent: 199999525 sats
Fee: 270 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed allowance steps: 12
Renewed policy PSBTs: 28
Emergency access preserved: 50000000 sats
Type `approve` to confirm the phone-key rotation: approve
HWW validated and signed the phone-key rotation plus 28 renewed-policy PSBTs
HWW-approved phone-key rotation: approved-phone-rotation.json

$ anzen phone activate-rotation approved-phone-rotation.json
Emergency phone-key rotation broadcast: 9a69e75861130dc229c2c047cb01682476a1a5dfda5293ad07c270c8044cbc4d
Old vault address: bcrt1pttgv9t0kfkrkfqfj6l3uu3gm5u5snu8w8a04zvqlkjd8meh3ugdqg3wsrw
New vault address: bcrt1pmdc3g35t8q4kmvj8vytcqswxwaz6djma8v9gl67upg4enkqwsmeq3qfmx3
New phone mnemonic: tragic diagram company search photo luggage claim manage element half border end rapid eagle solve brass off mesh pass select choice nice wing dune
Monthly policy preserved: 10000000 sats
Policy rollover broadcast: 77dd21c82ca5a349359464d073a39d21e2ed12d22e0c8459c1de50645081b596
Encrypted allowance transaction pairs: 12
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

The named tests cover setup/policy, sequential monthly spend, whole-chain monthly revoke, successful and cancelled one-week emergency access, partial funding, lost or stolen phone, lost or stolen HWW, missing cloud backup, both devices lost, OpenPGP social recovery, cloud compromise, both keys compromised, and both on-time and forgotten annual rollover. The spend demonstrations fund exactly 2 BTC and create one chain funding up to twelve 0.1 BTC allowance releases.

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
