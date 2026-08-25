<p align="center"><img src="media/anzen-banner-v2.svg" alt="Anzen" width="100%"></p>

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

This is Anzen's only principal-holding script. Monthly allowances and emergency access are implemented with presigned transaction chains using Bitcoin-enforced relative timelocks.

Revocable authorization state lives in separate 1,000-sat connector outputs. They reuse the same device keys under a fixed 1-of-2 Taproot policy:

```text
tr(NUMS,{pk(phone),pk(hww)})
```

Either device can spend a connector alone. The connector does not control vault principal; its exact outpoint is an execution token committed into a presigned policy PSBT.

### Annual vault layout and presigned transaction graph

Once per year, the phone proposes a policy and the HWW approves it once. They fully sign the rollover and sign only the vault input of each future action. Only the rollover is broadcast immediately. After it confirms, the vault has one allowance-chain UTXO, one remainder UTXO, and one small connector for each enabled independent action chain. Future action PSBTs remain incomplete and encrypted on the phone until it signs the current connector at execution.

![The sequential allowance chain and emergency transactions for an example 2.1 BTC annual vault policy](media/vault-utxo-layout.svg)

The overview shows the state immediately after rollover. Large blue circles are principal-holding vault UTXOs, small amber circles are the controller-wallet connectors, and boxes are transactions. A dashed outline means that output or transaction has not happened yet. Round principal amounts are shown so the policy is easy to read; the checked-in test vector contains exact fees and values.

#### Monthly allowance

![Twelve sequential monthly allowances with a connector rolling alongside the vault state, plus dynamic revocation](media/monthly-allowance-chain.svg)

Each monthly transaction has exactly two inputs: the current allowance vault UTXO and the current connector. After roughly 30 days, the phone signs the connector input and releases 0.1 BTC. Except for month twelve, that transaction creates both the next smaller vault UTXO and the exact connector required by the next step. Spending the live connector in a dynamic revocation leaves the current vault UTXO untouched and makes the current and every dependent future allowance impossible.

#### Emergency access

![Emergency access trigger, delayed withdrawal, and connector-only cancellation](media/emergency-access-chain.svg)

The immediate trigger spends the vault remainder and emergency connector, then creates a staged 0.5 BTC vault output, cold change, and the withdrawal connector. After one week, the staged output and connector can be spent together to the hot wallet. Before then, either device can cancel by spending only the withdrawal connector; both principal outputs remain under the vault script and the delayed withdrawal becomes permanently invalid.

Connector change never returns to the controller address: phone revocation sends it to a fresh hot-wallet change address, while HWW revocation requires an explicitly displayed destination. A later annual rollover spends every live vault and connector output, resets recovery delays, creates only the renewed policy's connectors, and invalidates the old epoch.

Presigned transactions are convenience permissions, not custody. Losing them cannot lose the bitcoin because every principal output still has the three vault-script paths above.

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
Phone mnemonic: hospital suit remain guard kidney trial task hope arrow catch shoe ceiling pole tattoo space fatigue lens wrist narrow guess cruise rail riot concert
Phone vault key: b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0

$ anzen hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: switch announce harsh welcome cotton bike grace polar rug welcome scatter exercise lounge couch box parrot orchard ship execute dolphin defy fuel quick girl
HWW vault key: e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f
HWW ready to wrap the descriptor-bound cloud backup at anzen init

$ anzen init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0,e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f),{and_v(v:older(61200),pk(b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0)),and_v(v:older(65535),pk(e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f))}})#ezseuq69
Vault address: bcrt1pk2xcl2m8p2kkwde8gq3tazx94ln8e9wxj49llakmz50zfg9md90qwug5ge
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Emergency access: disabled
Cloud recovery backup: phone key + descriptor encrypted; 0 recovery friends

$ anzen policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0,e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f),{and_v(v:older(61200),pk(b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0)),and_v(v:older(65535),pk(e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f))}})#ezseuq69
Vault address: bcrt1pk2xcl2m8p2kkwde8gq3tazx94ln8e9wxj49llakmz50zfg9md90qwug5ge
Policy controller address: bcrt1pvn6qsn0t7u4q02vaf2wku8cnfrfns29qetz743fgvwhmuk3akers6ca203
Policy controller reserve: 1000 sats per active chain
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Emergency access: disabled
```

The new vault starts with monthly spending and emergency access disabled. `anzen init` prints the static cold-storage descriptor, vault address, and recovery delays, but does not create or sign a spending policy.

### Set or replace the vault policy

The phone proposes the policy and signs its side of each vault input. The HWW independently reconstructs the graph, asks for one approval, and signs the other vault side. Future controller inputs stay unsigned until the phone executes an action, so neither device commits revocation authority into the annual package. This real regtest policy combines a 0.1 BTC monthly limit with one cancellable 0.5 BTC emergency withdrawal:

```console
$ anzen phone set-policy --monthly-limit 10000000 --emergency-access-limit 50000000 --output policy.json
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0,e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f),{and_v(v:older(61200),pk(b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0)),and_v(v:older(65535),pk(e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f))}})#ezseuq69
Vault address: bcrt1pk2xcl2m8p2kkwde8gq3tazx94ln8e9wxj49llakmz50zfg9md90qwug5ge
Policy controller address: bcrt1pvn6qsn0t7u4q02vaf2wku8cnfrfns29qetz743fgvwhmuk3akers6ca203
Policy controller reserve: 1000 sats per active chain
Monthly limit: 10000000 sats
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
Fee rate: 1 sat/vB
Total input: 200000000 sats
Allowance steps: 12
Allowance hop delay: 2592256 seconds (~30 days)
Rollover txid: 12e6e10746c37364348396b10b1865d0109a98c8cc58d68826903235fdc147d3
Rollover fee: 291 sats
Initial allowance-chain UTXO: 119993886 sats
Rollover remainder: 79985823 sats
Emergency trigger txid: d13232941c779472ac175bc6587c7b674e975e2f859efe5a32b810ec4637dd30
Emergency withdrawal txid: df9a8ae1d948d7876f3a5db2e0fb1e5529a94f383ec291649f4302b1abeb94c3
Emergency hot address: bcrt1p7yj6se5q6crpqasfpn3f6v37qh7agejts65q3tj5mtfrfjlk6fsqh42e9p
Phone signed PSBTs: 15
Phone-signed policy proposal: policy.json

$ anzen hww confirm-policy policy.json --output approved-policy.json --yes
SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0,e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f),{and_v(v:older(61200),pk(b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0)),and_v(v:older(65535),pk(e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f))}})#ezseuq69
Vault address: bcrt1pk2xcl2m8p2kkwde8gq3tazx94ln8e9wxj49llakmz50zfg9md90qwug5ge
Policy controller address: bcrt1pvn6qsn0t7u4q02vaf2wku8cnfrfns29qetz743fgvwhmuk3akers6ca203
Policy controller reserve: 1000 sats per active chain
Monthly limit: 10000000 sats
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
Fee rate: 1 sat/vB
Total input: 200000000 sats
Allowance steps: 12
Allowance hop delay: 2592256 seconds (~30 days)
Rollover txid: 12e6e10746c37364348396b10b1865d0109a98c8cc58d68826903235fdc147d3
Rollover fee: 291 sats
Initial allowance-chain UTXO: 119993886 sats
Rollover remainder: 79985823 sats
Emergency trigger txid: d13232941c779472ac175bc6587c7b674e975e2f859efe5a32b810ec4637dd30
Emergency withdrawal txid: df9a8ae1d948d7876f3a5db2e0fb1e5529a94f383ec291649f4302b1abeb94c3
Emergency hot address: bcrt1p7yj6se5q6crpqasfpn3f6v37qh7agejts65q3tj5mtfrfjlk6fsqh42e9p
Phone signed PSBTs: 15
HWW validated and signed all 15 PSBTs after one approval
HWW-approved policy: approved-policy.json

$ anzen phone activate-policy approved-policy.json
Rollover broadcast: 12e6e10746c37364348396b10b1865d0109a98c8cc58d68826903235fdc147d3
Active monthly limit: 10000000 sats
Encrypted allowance authorizations: 12
Active emergency access: 50000000 sats
Encrypted emergency transaction set: trigger, withdrawal

$ anzen policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0,e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f),{and_v(v:older(61200),pk(b336b46856b0c7dd2e2a2d4ffde1d1e7788707cfd246ebb72bebd395a19fdaf0)),and_v(v:older(65535),pk(e905d6034668f206e34cbba40808f94dfc676f88a19930160239ee14aaef445f))}})#ezseuq69
Vault address: bcrt1pk2xcl2m8p2kkwde8gq3tazx94ln8e9wxj49llakmz50zfg9md90qwug5ge
Policy controller address: bcrt1pvn6qsn0t7u4q02vaf2wku8cnfrfns29qetz743fgvwhmuk3akers6ca203
Policy controller reserve: 1000 sats per active chain
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly limit: 10000000 sats
Presigned allowance authorizations: 12
Allowance hop delay: 2592256 seconds (~30 days)
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
```

`10000000` sats is 0.1 BTC and `50000000` sats is 0.5 BTC. The policy contains 15 PSBTs: rollover, twelve allowance authorizations, emergency trigger, and emergency withdrawal. It does not contain presigned revocations or cancellation. Set either limit to zero through the same three-step protocol to disable that feature. Policy JSON may also be piped with `--output -`; file handoff is clearer for the interactive HWW approval.

### Execute a monthly spend

Allowances are numbered sequentially. Step 1 becomes valid roughly 30 days after rollover confirmation; each later step becomes valid roughly 30 days after the preceding authorization confirms. The exact BIP68 delay is 2,592,256 seconds:

```console
$ anzen phone authorize 1
Broadcast Authorization for allowance step 1: 7261aad194701449239640b059f6a77ba926c2a82e4e18a23947fddb14bd9428
```

The authorization releases the approved amount at output 0, creates step 2's vault output at output 1, and rolls the 1,000-sat connector to output 2. The phone adds only the controller signature at execution. Step 2 cannot mature before step 1 confirms, even if the phone waited much longer than 30 days before using step 1.

To keep only a 0.01 BTC soft limit from a 0.1 BTC authorization, immediately return the difference to cold storage:

```console
$ anzen phone apply-soft-limit 1 --limit 1000000
Soft limit applied for allowance step 1: retained at most 1000000 sats hot; cold-return txid=a24f6d06790f3ac08d5649f8820b14da9d0bd7aba108ce321ee552e2d7fae865
```

The signed monthly limit is the security boundary. The adjustable soft limit is a phone-side action and may be any value from zero through the signed monthly limit.

### Revoke all remaining monthly spends

Once a hop's source output exists, the phone can revoke it without the HWW. This transaction is built dynamically, spends only that hop's connector, and sends its remaining connector value to fresh hot-wallet change:

```console
$ anzen phone revoke 2
Broadcast Revocation for allowance step 2: 8abf7bd7d1d6d0372c23329a00011d5c6bdb0ba75f079afba02ffe675cdaabba
```

The step 2 vault output does not move. Once revocation confirms, its authorization cannot spend the already-consumed connector. Steps 3–12 are also invalid because they depend on outputs that step 2 can no longer create. Revocation is therefore deliberately whole-chain rather than per-allowance.

The HWW can invalidate every live policy chain in one action and sends the small connector remainder to an address shown for explicit approval:

```console
$ anzen hww revoke-policy bcrt1py8xc780ra4cads2lt4e9pawdy9hqlkmu8kzt58tqqgq9x7u7perqf7n0x9 --yes
SIMULATED HWW — REVOKE ACTIVE VAULT POLICY
Controller outputs: 2
Change destination: bcrt1py8xc780ra4cads2lt4e9pawdy9hqlkmu8kzt58tqqgq9x7u7perqf7n0x9
All policy transactions committed to these states will be invalidated
HWW policy revocation broadcast: 0a1cfab1fc7be1f337430a95b2274315fc0abef4d000369735d4bd967c5f5564
Revoked controller outputs: 2
```

### Use or cancel emergency access

The policy authorizes one emergency trigger per vault epoch. Starting it spends the rollover remainder into a staging output plus cold change and begins the Bitcoin-enforced cancellation window:

```console
$ anzen phone emergency initiate
Emergency access initiated: d13232941c779472ac175bc6587c7b674e975e2f859efe5a32b810ec4637dd30
Amount after delay: 50000000 sats
Cancellation window: 605184 seconds

$ anzen phone emergency withdraw
Error: failed to broadcast emergency access Withdrawal
```

After the trigger confirms and the one-week BIP68 delay elapses, the same command releases exactly the approved amount to the fresh hot-wallet address committed by the policy:

```console
$ anzen phone emergency withdraw
Emergency access withdrawal broadcast: df9a8ae1d948d7876f3a5db2e0fb1e5529a94f383ec291649f4302b1abeb94c3
```

Alternatively, the phone can cancel before maturity. These outputs are from the isolated cancellation test's own vault epoch:

```console
$ anzen phone emergency initiate
Emergency access initiated: 85fe845dfb9967b0468e32b012dcc4ea93329a952ee707fd9d1e1af67d9d58cd
Amount after delay: 50000000 sats
Cancellation window: 605184 seconds

$ anzen phone emergency cancel
Emergency access cancelled: 22e3f2066b72042c8975b26debca1f048aea3f0682ca3504ee0b0b756c7b1e55

$ anzen phone emergency withdraw
Error: failed to broadcast emergency access Withdrawal
```

Cancellation is constructed dynamically and spends only the withdrawal connector to fresh hot-wallet change. The staged 0.5 BTC remains in its existing vault output. Once cancellation confirms, the delayed withdrawal remains invalid even after its timelock expires because it committed to that exact connector outpoint. BIP68 uses 512-second units, so the enforced minimum is 605,184 seconds—seven days plus 6 minutes 24 seconds.

### Replace a lost phone

If the encrypted cloud backup survives, the HWW decrypts it into a portable recovery object. After installing that object on the replacement phone, rotate immediately to a fresh phone key and vault address:

```console
$ anzen hww decrypt-phone-backup \
  .anzen-data/cloud/phone-seed-backup.json \
  --output phone-recovery.json
Decrypted phone recovery package: phone-recovery.json

$ anzen phone restore phone-recovery.json
Phone key restored from authenticated recovery package
Recovered phone mnemonic: fade note doctor brass obey increase foam surprise volcano coin cliff square have effort cover own ride ghost poem exact hungry kick lounge minute

$ anzen phone rotate-key --output phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: d3e1b1147aabd479268dc03c34c9ce3ca9f9265f050b14de50df6dfaaf00d52c
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,d3e1b1147aabd479268dc03c34c9ce3ca9f9265f050b14de50df6dfaaf00d52c,026f7045ba3eef34e191d16e0a18dcaec240d876b76e9f5f683ab9e685eeb034),{and_v(v:older(61200),pk(d3e1b1147aabd479268dc03c34c9ce3ca9f9265f050b14de50df6dfaaf00d52c)),and_v(v:older(65535),pk(026f7045ba3eef34e191d16e0a18dcaec240d876b76e9f5f683ab9e685eeb034))}})#jge8gjrv
New vault address: bcrt1pzpkjnt5v2csz9xslg5yacu4qtnqaxlctngdavztryfm2t2hd7gfqhe7a39
Inputs: 2
Sent: 199979439 sats
Fee: 270 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed allowance steps: 12
Renewed policy PSBTs: 15
Emergency access preserved: 50000000 sats
Phone-key rotation proposal: phone-rotation.json

$ anzen hww confirm-rotation phone-rotation.json \
  --output approved-phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: d3e1b1147aabd479268dc03c34c9ce3ca9f9265f050b14de50df6dfaaf00d52c
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,d3e1b1147aabd479268dc03c34c9ce3ca9f9265f050b14de50df6dfaaf00d52c,026f7045ba3eef34e191d16e0a18dcaec240d876b76e9f5f683ab9e685eeb034),{and_v(v:older(61200),pk(d3e1b1147aabd479268dc03c34c9ce3ca9f9265f050b14de50df6dfaaf00d52c)),and_v(v:older(65535),pk(026f7045ba3eef34e191d16e0a18dcaec240d876b76e9f5f683ab9e685eeb034))}})#jge8gjrv
New vault address: bcrt1pzpkjnt5v2csz9xslg5yacu4qtnqaxlctngdavztryfm2t2hd7gfqhe7a39
Inputs: 2
Sent: 199979439 sats
Fee: 270 sats (1 sat/vB)
Monthly policy preserved: 10000000 sats
Renewed allowance steps: 12
Renewed policy PSBTs: 15
Emergency access preserved: 50000000 sats
Type `approve` to confirm the phone-key rotation: approve
HWW validated and signed the phone-key rotation plus 15 renewed-policy PSBTs
HWW-approved phone-key rotation: approved-phone-rotation.json

$ anzen phone activate-rotation approved-phone-rotation.json
Emergency phone-key rotation broadcast: 6be491fa9422a51e2bf9a95e750d3e365c7834e3d6313fffdfdaa7b241828233
Old policy controllers revoked: 1451a65930c91233c2650aa563f7821bd63e12f97aa1698feed6cfb7a1372bc8
Old vault address: bcrt1p546pmuhh7f25ra5pcvrt83kmuq6jcgn98jag5ns7tjnzf2zceezq0nt38x
New vault address: bcrt1pzpkjnt5v2csz9xslg5yacu4qtnqaxlctngdavztryfm2t2hd7gfqhe7a39
New phone mnemonic: border video ice witness wash abstract genuine artefact pioneer zoo exile seat panel dutch chapter you swallow horse attitude damp legal six spray desk
Monthly policy preserved: 10000000 sats
Policy rollover broadcast: 9a2982f112b2e522ae70141732de9b70ab46b641e2ccbacdd225e436d35d622a
Encrypted allowance authorizations: 12
Emergency access preserved: 50000000 sats
```

The rotation preserves the HWW key, every configured recovery friend, and the active monthly and emergency-access limits. Before installing the new phone key it spends every old controller to replacement-phone change, so no old-key policy authority survives. It then creates a new phone seed and descriptor-bound cloud envelope and sweeps the old vault cooperatively. The same proposal chains a fresh annual policy to that sweep; one HWW prompt approves both, and all replacement artifacts and connectors use the new phone key. A disabled feature remains disabled after rotation.

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
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Project layout, development commands, and the hardware-firmware workflow are documented in [DEVELOPMENT.md](DEVELOPMENT.md). Implementation rationale and engineering trade-offs are recorded in [design-decisions.md](design-decisions.md).

## License

[MIT](LICENSE) © 2026 Luke Childs
