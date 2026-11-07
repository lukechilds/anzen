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

These examples use disposable regtest wallets. The simulated devices print their mnemonics for demonstration. The outputs are copied from the [end-to-end tests](#run-the-end-to-end-tests), where every scenario runs against its own fresh vault, so keys, addresses, and txids differ between sections.

### Create a vault

Initialize each simulated device separately, then run `anzen init` to pair them, combine their public keys into the static cold-storage policy, and store the first encrypted cloud backup:

For mainnet, pass `--dangerously-enable-mainnet` to every command. Mainnet defaults to Electrum over WebSockets (`wss://electrum.lu.ke`); use `--chain-backend rpc` for your own Bitcoin Core node, or `--electrum-url wss://…` for your own Electrum server.

```console
$ anzen phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: mistake squeeze aspect thought achieve chase glow slush deposit approve front trim dream salmon wide ice green window farm jacket pitch enemy where roast
Phone vault key: 88fcdf6472fdce18d671d72e3cd3262e38275b15d356482001049b5a57c351b9

$ anzen hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: sick kidney judge hold spoil april rebuild deliver pyramid bone suffer warm drama ecology evidence width page wine smoke insane note limit off deputy
HWW vault key: 1f6ac05a5a5b6ef31c8ab20fa692e327b43e25532d03853f5563108c6d9da33c
HWW ready to pair with the phone and wrap its cloud backup key at anzen init

$ anzen init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,88fcdf6472fdce18d671d72e3cd3262e38275b15d356482001049b5a57c351b9,1f6ac05a5a5b6ef31c8ab20fa692e327b43e25532d03853f5563108c6d9da33c),{and_v(v:older(61200),pk(88fcdf6472fdce18d671d72e3cd3262e38275b15d356482001049b5a57c351b9)),and_v(v:older(65535),pk(1f6ac05a5a5b6ef31c8ab20fa692e327b43e25532d03853f5563108c6d9da33c))}})#j5r7gp0v
Vault address: bcrt1pfh6f4e6clj79g0qahzl7g9jv5e7rhgl7keq0k2939hk86rj7jzkszwz5hl
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Emergency access: disabled
Cloud recovery backup: phone key + descriptor encrypted; 0 recovery friends

$ anzen policy
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,88fcdf6472fdce18d671d72e3cd3262e38275b15d356482001049b5a57c351b9,1f6ac05a5a5b6ef31c8ab20fa692e327b43e25532d03853f5563108c6d9da33c),{and_v(v:older(61200),pk(88fcdf6472fdce18d671d72e3cd3262e38275b15d356482001049b5a57c351b9)),and_v(v:older(65535),pk(1f6ac05a5a5b6ef31c8ab20fa692e327b43e25532d03853f5563108c6d9da33c))}})#j5r7gp0v
Vault address: bcrt1pfh6f4e6clj79g0qahzl7g9jv5e7rhgl7keq0k2939hk86rj7jzkszwz5hl
Policy controller address: bcrt1pxu6dxg6pmryuqtuu39ukf56d5ydqa72gv05z23cyyxc65q9cdn3sxe46p7
Policy controller reserve: 1000 sats per active chain
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
Emergency access: disabled
```

The new vault starts with monthly spending and emergency access disabled. `anzen init` prints the static cold-storage descriptor, vault address, and recovery delays, and encrypts the phone key and descriptor into the cloud backup with its key wrapped for the HWW. It does not create or sign a spending policy.

### Set or replace the vault policy

The phone proposes the policy. The HWW independently reconstructs the graph, asks for one approval, and signs every presigned transaction the policy implies. The phone then verifies the approved policy, adds its own signatures, and broadcasts the rollover. Future controller inputs stay unsigned until the phone executes an action, so neither device commits revocation authority into the annual package. This real regtest policy combines a 0.1 BTC monthly limit with one cancellable 0.5 BTC emergency withdrawal:

```console
$ anzen phone set-policy --monthly-limit 10000000 --emergency-access-limit 50000000 --output policy.json
PHONE POLICY PROPOSAL
Vault address: bcrt1plgu8x9xp7539wthvz6qps5hz4vwg45ajedr3v0cwglxen308r73qf9g87q
Policy controller address: bcrt1pgc79k68sj02rpdur8438rtmg9vadckhwf2dy5fkq5qyu4q49m7cs9zm2ly
Policy controller reserve: 1000 sats per active chain
Monthly limit: 10000000 sats
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
Fee rate: 1 sat/vB
Total input: 200000000 sats
Allowance steps: 12
Allowance hop delay: 2592256 seconds (~30 days)
Rollover txid: 15c2b85b2cd22f5aa93d064b6118ca1d2c73ee4d649742ecab435f4722b985c3
Rollover fee: 291 sats
Initial allowance-chain UTXO: 120002886 sats
Rollover remainder: 79994823 sats
Emergency trigger txid: a65992ce96d2c306f5548d66eff46f798c2068589909836dffeb228732281eff
Emergency withdrawal txid: 5d5de9c172b7fe9fa88f794ce783bc20e70ddc790ca4c549faecae6b9c97a1c4
Emergency hot address: bcrt1pty2qhk9z39m26c6450rm89yzptcgwn8u7km4eruzjzncl8vxljmq6qcz78
Presigned transactions: 15
Policy proposal: policy.json

$ anzen hww confirm-policy policy.json --output approved-policy.json --yes
SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL
Policy controller address: bcrt1pgc79k68sj02rpdur8438rtmg9vadckhwf2dy5fkq5qyu4q49m7cs9zm2ly
Policy controller reserve: 1000 sats per active chain
Monthly limit: 10000000 sats
Emergency access limit: 50000000 sats
Emergency access delay: 605184 seconds (~1 week)
Allowance steps: 12
Allowance hop delay: 2592256 seconds (~30 days)
Rollover txid: 15c2b85b2cd22f5aa93d064b6118ca1d2c73ee4d649742ecab435f4722b985c3
Initial allowance-chain UTXO: 120002886 sats
Rollover remainder: 79994823 sats
Emergency trigger txid: a65992ce96d2c306f5548d66eff46f798c2068589909836dffeb228732281eff
Emergency withdrawal txid: 5d5de9c172b7fe9fa88f794ce783bc20e70ddc790ca4c549faecae6b9c97a1c4
Emergency hot address: bcrt1pty2qhk9z39m26c6450rm89yzptcgwn8u7km4eruzjzncl8vxljmq6qcz78
Presigned transactions: 15
HWW validated and signed all 15 presigned transactions after one approval
HWW-approved policy: approved-policy.json

$ anzen phone activate-policy approved-policy.json
Rollover broadcast: 15c2b85b2cd22f5aa93d064b6118ca1d2c73ee4d649742ecab435f4722b985c3
Active monthly limit: 10000000 sats
Encrypted allowance authorizations: 12
Active emergency access: 50000000 sats
Encrypted emergency transaction set: trigger, withdrawal

$ anzen status
Network: regtest
Height: 103
Vault UTXOs: 2
Vault balance: 199997709 sats
Live policy controller outputs: 2
```

`10000000` sats is 0.1 BTC and `50000000` sats is 0.5 BTC. The policy contains 15 presigned transactions: rollover, twelve allowance authorizations, emergency trigger, and emergency withdrawal. With emergency access disabled it contains 13. It does not contain presigned revocations or cancellation. Set either limit to zero through the same three-step protocol to disable that feature. Policy JSON may also be piped with `--output -`; file handoff is clearer for the interactive HWW approval.

### Execute a monthly spend

Allowances are numbered sequentially. Step 1 becomes valid roughly 30 days after rollover confirmation; each later step becomes valid roughly 30 days after the preceding authorization confirms. The exact BIP68 delay is 2,592,256 seconds. These outputs are from the monthly-spend test's own vault:

```console
$ anzen phone authorize 1
Broadcast Authorization for allowance step 1: 34b53f1f68d1f7ec0bcbf7d863d6291adedff029f531a6bb18519a01f42671de
```

The authorization releases the approved amount at output 0, creates step 2's vault output at output 1, and rolls the 1,000-sat connector to output 2. The phone adds only the controller signature at execution. Step 2 cannot mature before step 1 confirms, even if the phone waited much longer than 30 days before using step 1.

To keep only a 0.01 BTC soft limit from a 0.1 BTC authorization, immediately return the difference to cold storage:

```console
$ anzen phone apply-soft-limit 1 --limit 1000000
Soft limit applied for allowance step 1: retained at most 1000000 sats hot; cold-return txid=ee3084bcdfd19636270418d6729d1bd07fce17a425e1aac1c24b929eb332d474
```

The signed monthly limit is the security boundary. The adjustable soft limit is a phone-side action and may be any value from zero through the signed monthly limit.

### Revoke all remaining monthly spends

Once a hop's source output exists, the phone can revoke it without the HWW. This transaction is built dynamically, spends only that hop's connector, and sends its remaining connector value to fresh hot-wallet change. This output is from the monthly-revoke test's own vault:

```console
$ anzen phone revoke 2
Broadcast Revocation for allowance step 2: 463d21ce506d662668b0679cdcaa95447a2b4bd541efdafea5b80cdad43b1eae
```

The step 2 vault output does not move. Once revocation confirms, its authorization cannot spend the already-consumed connector. Steps 3–12 are also invalid because they depend on outputs that step 2 can no longer create. Revocation is therefore deliberately whole-chain rather than per-allowance.

The HWW can invalidate every live policy chain in one action and sends the small connector remainder to an address shown for explicit approval. This continues the vault from the policy example above:

```console
$ anzen hww revoke-policy bcrt1p8gs335mr9ymzecg4p3savt5tme5gtjsu0aggp0vxpkaz32eh352qs7q60l --yes
SIMULATED HWW — REVOKE ACTIVE VAULT POLICY
Controller outputs: 2
Change destination: bcrt1p8gs335mr9ymzecg4p3savt5tme5gtjsu0aggp0vxpkaz32eh352qs7q60l
All policy transactions committed to these states will be invalidated
HWW policy revocation broadcast: 7614abc927b2f3badbd262b3a0600c9dd419d9d08079ccc151a082029ad230e4
Revoked controller outputs: 2
```

### Use or cancel emergency access

The policy authorizes one emergency trigger per vault epoch. Starting it spends the rollover remainder into a staging output plus cold change and begins the Bitcoin-enforced cancellation window. These outputs are from the emergency-access test's own vault epoch:

```console
$ anzen phone emergency initiate
Emergency access initiated: 04df27cbd9d30eabcfce09b9c85e3868728b88a3e2ddd0b364f96716727393ad
Amount after delay: 50000000 sats
Cancellation window: 605184 seconds

$ anzen phone emergency withdraw
Error: failed to broadcast emergency access Withdrawal
```

After the trigger confirms and the one-week BIP68 delay elapses, the same command releases exactly the approved amount to the fresh hot-wallet address committed by the policy:

```console
$ anzen phone emergency withdraw
Emergency access withdrawal broadcast: b99ea91e50181f5b4aabee1b86a19a4bd3b81ecb6cffdf335a8792b6d45b0f04
```

Alternatively, the phone can cancel before maturity. These outputs are from the isolated cancellation test's own vault epoch:

```console
$ anzen phone emergency initiate
Emergency access initiated: f0d54450da77bde54e6652b8b32fc72f2c0daf3c00e49630e65cded9bec5f3f9
Amount after delay: 50000000 sats
Cancellation window: 605184 seconds

$ anzen phone emergency cancel
Emergency access cancelled: c190a5b6b795002860c0872932988c228e1e60c70b52558b9d9ecb41ff6e766d

$ anzen phone emergency withdraw
Error: failed to broadcast emergency access Withdrawal
```

Cancellation is constructed dynamically and spends only the withdrawal connector to fresh hot-wallet change. The staged 0.5 BTC remains in its existing vault output. Once cancellation confirms, the delayed withdrawal remains invalid even after its timelock expires because it committed to that exact connector outpoint. BIP68 uses 512-second units, so the enforced minimum is 605,184 seconds—seven days plus 6 minutes 24 seconds.

### Replace a lost phone

If the encrypted cloud backup survives, initialize the replacement phone with a fresh key and import the vault from an HWW recovery bundle. The phone then asks the HWW to release the cloud backup's key to that fresh key; the HWW shows the replacement key for comparison and never sees the phone seed. After restoring, rotate immediately to the replacement key and a new vault address, then pair the HWW with the rotated vault. These outputs are from the lost-phone test's own vault:

```console
$ anzen phone init
Simulated phone initialized (REGTEST ONLY)
Phone vault key: 04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794

$ anzen hww export-recovery --output recovery-bundle.json
Vault address: bcrt1pptj7234wglqrzd6s27q9vquney29cpjavlsqau7eu7a7ukpqt0sqv297vl
HWW recovery bundle: recovery-bundle.json

$ anzen phone import-recovery recovery-bundle.json
Recovery vault imported: bcrt1pptj7234wglqrzd6s27q9vquney29cpjavlsqau7eu7a7ukpqt0sqv297vl

$ anzen phone restore-request .anzen-data/cloud/phone-seed-backup.json --output restore-request.json
Restore request for vault bcrt1pptj7234wglqrzd6s27q9vquney29cpjavlsqau7eu7a7ukpqt0sqv297vl
Replacement phone key: 04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794
Restore request: restore-request.json

$ anzen hww approve-restore restore-request.json --output restore-response.json --yes
SIMULATED HWW — RESTORE A LOST PHONE
Vault address: bcrt1pptj7234wglqrzd6s27q9vquney29cpjavlsqau7eu7a7ukpqt0sqv297vl
Replacement phone key: 04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794
Compare this key with the replacement phone before approving
HWW released the cloud backup key to the replacement phone
HWW restore response: restore-response.json

$ anzen phone restore restore-response.json
Phone key restored from the authenticated cloud backup
Recovered phone key: c03280cffc72741b82ddc91de1ca38998d51d7b148b8a21b2a8de0102fdfe411
Replacement phone key: 04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794
Rotate the vault to the replacement key before using it

$ anzen phone rotate-key --output phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: 04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794,39e34b45151097baa5bf11460423f84104cc48c033abc95c20d7ce8c392c16df),{and_v(v:older(61200),pk(04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794)),and_v(v:older(65535),pk(39e34b45151097baa5bf11460423f84104cc48c033abc95c20d7ce8c392c16df))}})#xl7wedk6
New vault address: bcrt1pe64zjpdwaepe53jht2vs2c9ux0k7yen6tfhwv07ce598qw0w3urs446krl
Inputs: 2
Sent: 199997439 sats
Fee: 270 sats (1 sat/vB)
Old policy controllers revoked: 2
Monthly policy preserved: 10000000 sats
Renewed allowance steps: 12
Renewed policy transactions: 15
Emergency access preserved: 50000000 sats
Phone-key rotation proposal: phone-rotation.json

$ anzen hww confirm-rotation phone-rotation.json --output approved-phone-rotation.json --yes
PHONE-KEY ROTATION
New phone vault key: 04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794
New cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794,39e34b45151097baa5bf11460423f84104cc48c033abc95c20d7ce8c392c16df),{and_v(v:older(61200),pk(04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794)),and_v(v:older(65535),pk(39e34b45151097baa5bf11460423f84104cc48c033abc95c20d7ce8c392c16df))}})#xl7wedk6
New vault address: bcrt1pe64zjpdwaepe53jht2vs2c9ux0k7yen6tfhwv07ce598qw0w3urs446krl
Inputs: 2
Sent: 199997439 sats
Fee: 270 sats (1 sat/vB)
Old policy controllers revoked: 2
Monthly policy preserved: 10000000 sats
Renewed allowance steps: 12
Renewed policy transactions: 15
Emergency access preserved: 50000000 sats
HWW validated and signed the phone-key rotation plus 15 renewed-policy transactions
HWW-approved phone-key rotation: approved-phone-rotation.json

$ anzen phone activate-rotation approved-phone-rotation.json
Phone-key rotation sweep broadcast: af1dfdf75928664ae14064089f8a6f30ce14efc8dbee3b88e98162e2229a5ec1
Old policy controllers revoked: 87645cff157e4c8d0cce75d82829a5ac13e6b5167741d96ea194e6b2ec277a00
Old vault address: bcrt1pptj7234wglqrzd6s27q9vquney29cpjavlsqau7eu7a7ukpqt0sqv297vl
New vault address: bcrt1pe64zjpdwaepe53jht2vs2c9ux0k7yen6tfhwv07ce598qw0w3urs446krl
Phone mnemonic: room absurd inject pole depth exit tenant wisdom cook predict net practice dry clarify panther album gossip million else maximum aim use they buyer
Monthly policy preserved: 10000000 sats
Policy rollover broadcast: 996bef84a3d995d2f791764f7f6d88987d4f86e8fe371c33996639e3b33cb7ad
Encrypted allowance authorizations: 12
Emergency access preserved: 50000000 sats

$ anzen hww adopt-rotation approved-phone-rotation.json
HWW paired with the rotated vault: bcrt1pe64zjpdwaepe53jht2vs2c9ux0k7yen6tfhwv07ce598qw0w3urs446krl
```

The rotation preserves the HWW key, every configured recovery friend, and the active monthly and emergency-access limits. Before installing the new phone key it spends every old controller to replacement-phone change, so no old-key policy authority survives. It then sweeps the old vault cooperatively to a descriptor built from the replacement phone key and writes a new descriptor-bound cloud backup. The same proposal chains a fresh annual policy to that sweep; one HWW prompt approves both, and all replacement artifacts and connectors use the new phone key. A disabled feature remains disabled after rotation.

### Configure social recovery

The backup payload contains the phone mnemonic and cold-storage descriptor, authenticated-encrypted under one random symmetric key. The HWW holds one encrypted copy of that key. Each recovery friend can hold another copy encrypted to their OpenPGP public key; the complete friend list is also authenticated by the symmetric key so cloud tampering cannot silently change who survives a later rotation. Friends are independent 1-of-N recipients, not threshold shares.

The key generator is a simulation convenience. In a real integration, import a public key whose private half stays under the friend's control. The phone proposes a new cloud backup that also wraps its key for the friend, the HWW shows the friend's fingerprint and wraps the key for itself, and the phone stores the approved backup. These outputs are from the social-recovery test's own vault:

```console
$ anzen social generate-friend-key --name "Alice <alice@example.test>" --public-key alice.pub.asc --private-key alice.sec.asc
Recovery friend OpenPGP key generated: bd4c176d8c2a7affd3270bc5f400724413ad6d52
Public key: alice.pub.asc
Private key: alice.sec.asc (give only to the recovery friend)

$ anzen phone add-recovery-friend alice.pub.asc --output backup-request.json
Recovery friend proposed: bd4c176d8c2a7affd3270bc5f400724413ad6d52
Cloud backup request: backup-request.json

$ anzen hww confirm-backup backup-request.json --output backup-response.json --yes
SIMULATED HWW — CLOUD BACKUP
Vault address: bcrt1puc46hha0ppy8h9ztz5v9c7kz60lkjr0k4d8dqdpet7p55sh6jqrqu6nep2
Recovery friend OpenPGP fingerprint: bd4c176d8c2a7affd3270bc5f400724413ad6d52
Each friend gains the phone key and descriptor if they obtain the cloud backup
The 61,200-block phone recovery delay still applies to vault funds
HWW wrapped the cloud backup key for 1 recovery friends
HWW-approved cloud backup: backup-response.json

$ anzen phone store-backup backup-response.json
Cloud recovery backup stored; 1 recovery friends
```

If both devices are lost, the friend can authenticate and decrypt the portable recovery package. The command displays the recovered public binding but writes the mnemonic only inside the private JSON output:

```console
$ anzen social decrypt-backup .anzen-data/cloud/phone-seed-backup.json --private-key alice.sec.asc --output friend-recovery.json
Social recovery decrypted and authenticated
Phone vault key: 8176ca6abcb671f0ed6287fe31a0e4616a2d71d439944a06cd582f9d2a7c0b75
Cold storage descriptor: tr(50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0,{multi_a(2,8176ca6abcb671f0ed6287fe31a0e4616a2d71d439944a06cd582f9d2a7c0b75,4198d7b1b65b81961896fc11bdf606bf0a0cf1ed1dc78084743b389102581184),{and_v(v:older(61200),pk(8176ca6abcb671f0ed6287fe31a0e4616a2d71d439944a06cd582f9d2a7c0b75)),and_v(v:older(65535),pk(4198d7b1b65b81961896fc11bdf606bf0a0cf1ed1dc78084743b389102581184))}})#rzggszg2
Vault address: bcrt1puc46hha0ppy8h9ztz5v9c7kz60lkjr0k4d8dqdpet7p55sh6jqrqu6nep2
Friend-decrypted phone recovery package: friend-recovery.json
```

Social recovery reconstructs `M`, not `H`, so it cannot bypass the vault script. After the real 61,200-block phone delay matures, the friend can sweep directly to a replacement wallet's address without installing either lost device:

```console
$ anzen social emergency-access .anzen-data/cloud/phone-seed-backup.json --private-key alice.sec.asc bcrt1p9gjlh9c6l9a3cglxkv7px5qkx4ge482qh0qce33dkqg8a28tn2ps2f62df
Social emergency-access sweep broadcast: c50ff399b4fd2eece258e9c716a5dade99f201b145bf43cc62af85ea5687bdee
Inputs: 1
Sent: 199999854 sats
Fee: 146 sats (1 sat/vB)
On-chain phone recovery delay was enforced
```

Possession of a configured friend's private key is eventual phone-key capability. The HWW must therefore show that trust expansion clearly before adding a friend. The MVP-generated private key is unencrypted on disk; production friend-key UX and threshold recovery remain future work.

Without a configured friend, losing both devices is final even if the encrypted cloud backup survives: a replacement phone can import the public vault configuration and build a restore request, but only the lost HWW can approve it, and no surviving key can sign either mature recovery path.

If the phone and its backup are permanently unavailable, initialize a replacement vault in a separate data directory (`phone init`, `hww init`, and `init` with `--data-dir .replacement-vault`), take a receive address from its phone, wait the real 65,535-block HWW delay, and recover directly to that address. These outputs are from the lost-phone-no-cloud test's own vault:

```console
$ anzen --data-dir .replacement-vault phone receive-address
Hot receive address: bcrt1p942dzxwt78787mqjl40t4wym6qnz42rculqrerh59flwdtflm2es57ksr4

$ anzen hww recover bcrt1p942dzxwt78787mqjl40t4wym6qnz42rculqrerh59flwdtflm2es57ksr4
HWW recovery sweep broadcast: c6cbf842d5e6e0489a377fdd1c56efc7ceff4f7d389720daba9d7235017c0548
Inputs: 1
Sent: 199999854 sats
Fee: 146 sats (1 sat/vB)
```

This delayed recovery moves the funds to the replacement wallet, whose phone and HWW keys are both new.

### Replace a lost HWW

The phone can continue using existing monthly artifacts while the recovery delay runs. Phone-only recovery stays locked until the real 61,200-block phone delay matures. These outputs are from the lost-hww test's own vault:

```console
$ anzen phone authorize 1
Broadcast Authorization for allowance step 1: 40c23fca2847a3266fad13cbf024f13b9b3e427d8f8ed5e08d16a88e48934e79

$ anzen phone receive-address
Hot receive address: bcrt1pmrf6lgkw87mvgdt6de8cwfhszljswh27fa7jxn25lhv5tasd5ndsav0l5z

$ anzen phone recover bcrt1pmrf6lgkw87mvgdt6de8cwfhszljswh27fa7jxn25lhv5tasd5ndsav0l5z
Error: no vault UTXOs are mature for PhoneRecovery; earliest next-block validity height is 61303 (61187 blocks remaining)
```

Once the delay matures, the same command sweeps every remaining vault output, including the unused allowance chain, to the phone's hot wallet:

```console
$ anzen phone recover bcrt1pmrf6lgkw87mvgdt6de8cwfhszljswh27fa7jxn25lhv5tasd5ndsav0l5z
Phone recovery sweep broadcast: 617ee633ebd340bfc42e5613f035a1bfea5024c6c6a299a0971321c1f690097c
Inputs: 2
Sent: 189998183 sats
Fee: 238 sats (1 sat/vB)
```

After the sweep the old vault is empty, so the missing HWW can no longer participate. Create a new vault with a replacement HWW before returning the funds to cold storage, then approve a new monthly policy.

### Other cooperative sweeps

Arbitrary immediate vault sweeps retain the same explicit device boundary. The phone proposes the sweep without signing it:

```console
$ anzen phone create-sweep bcrt1pmzwluktvjp6mvktghy7yp8pyervxzxjnzxauqdnuwvzfjjcy449svdaupv --output sweep.json
COOPERATIVE VAULT SWEEP
Destination: bcrt1pmzwluktvjp6mvktghy7yp8pyervxzxjnzxauqdnuwvzfjjcy449svdaupv
Inputs: 1
Sent: 199999838 sats
Fee: 162 sats (1 sat/vB)
Cooperative sweep proposal: sweep.json
```

The HWW then reviews and signs it with `anzen hww confirm-sweep sweep.json --output approved-sweep.json`, and `anzen phone broadcast-sweep approved-sweep.json` verifies the approval, adds the phone signature, and broadcasts. This proposal comes from the stolen-phone test, where the HWW is absent, so `confirm-sweep` fails and the vault stays put; the end-to-end tests do not show a completed cooperative sweep.

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
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
./scripts/check-boundaries.sh
```

Project layout, development commands, and the hardware-firmware workflow are documented in [DEVELOPMENT.md](DEVELOPMENT.md). Implementation rationale and engineering trade-offs are recorded in [design-decisions.md](design-decisions.md).

## License

[MIT](LICENSE) © 2026 Luke Childs
