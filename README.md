<p align="center"><img src="media/anzen-banner-v2.svg" alt="Anzen" width="100%"></p>

[![CI](https://github.com/lukechilds/anzen/actions/workflows/ci.yml/badge.svg)](https://github.com/lukechilds/anzen/actions/workflows/ci.yml)

> [!WARNING]
> Anzen is an experimental reference implementation, not a production-ready wallet. The current hardware wallet is simulated in software, mnemonics are printed, and fees are fixed at 1 sat/vB. Use regtest to explore the protocol; do not secure significant mainnet funds with this prototype.

Anzen is a Bitcoin wallet that achieves the two-device compromise resistance of a 2-of-3 multivendor multisig with the simplicity of a mobile hot wallet. It combines a phone hot wallet for everyday use with an independent hardware wallet and a programmable Taproot vault for cold storage.

Most self-custody designs force a compromise: singlesig is simple but makes one key a catastrophic point of failure; multivendor multisig removes that failure but is difficult to operate; collaborative custody restores good UX by introducing a provider with a signing key. Anzen is designed to be secure, easy to use, and trustless at the same time. [Read the motivation behind the design.](https://lu.ke/self-custody-trilemma/)

**The experience of a hot wallet, backed by cold storage.** Think checking and savings: the phone is your checking account and the vault is your savings account. Once a year, the hardware wallet approves a human-readable policy covering monthly transfers, immediate revocations, and a larger emergency transfer with a one-week cancellation window. It validates and presigns the complete transaction graph in one ceremony; the phone stores it encrypted and can execute the policy for the rest of the year.

Immediate vault access requires both independent device keys. The phone acts as a second factor: on its own it can only use the bounded allowance and emergency access the hardware wallet approved for the year, never the savings. The hardware wallet can move the savings alone only after a year without renewal, so a lost phone can never lock the bitcoin away. No company, server, custodian, or online cosigner can change those paths, and losing the presigned policy transactions cannot lose the underlying bitcoin.

Anzen is designed to be difficult to operate incorrectly. It guides complete policy, renewal, revocation, rotation, and recovery operations rather than exposing raw transaction machinery and leaving the user to assemble a safe procedure.

Anzen is an open protocol, and this repository is its reference wallet implementation. Any hardware wallet can implement the cold-wallet side, and any mobile or desktop wallet can implement the hot-wallet side. Compatible implementations can work together without the reference app or any particular vendor.

The [protocol design](anzen-design.md) specifies the exact wallet properties, transaction graph, assumptions, and loss or theft behavior. [Design decisions](design-decisions.md) explains why Anzen uses this model and records its engineering trade-offs.

## How the vault is constructed

Anzen's cold storage is a Taproot address controlled by two keys: the mobile key (`phone`) and the hardware-wallet key (`hww`). Every cold output uses the same vault script. Bitcoin accepts a spend through either of these paths:

```text
phone + hww immediately, as one MuSig2 signature through the key path
hww only after ~388 days, through the only script leaf
```

The joint path is the Taproot key path. Its internal key is the [BIP327](https://github.com/bitcoin/bips/blob/master/bip-0327.mediawiki) MuSig2 aggregate of both device keys, so a cooperative spend carries a single 64-byte signature that looks like any other single-key Taproot spend and reveals neither the scripts nor that two devices signed.

The delay is the longest relative timelock Bitcoin supports: 65,535 units of 512 seconds, 33,553,920 seconds. It belongs to each individual UTXO and counts from the median time past of the block before the one that confirmed it, so it lasts the same on every network. It is not controlled by Anzen, a server, or a calendar. The annual renewal moves every vault output, which restarts the delay; the hardware wallet's solo path only opens if a year's renewal is missed.

There is deliberately no phone-only path. Without the hardware wallet the phone holds a bounded allowance, not the savings: a lost signer is restored from its own maker's backup as the same key, and a stolen phone cannot outlast the signer.

### Miniscript policy

The policy in minimal Taproot Miniscript descriptor notation is:

```text
tr(
  musig(phone,hww),
  and_v(v:after(T),and_v(v:older(4259839),pk(hww)))
)
```

`musig(phone,hww)` is the BIP327 aggregate of the two keys' even-y points, taken in sorted order. The stored descriptor writes out the resulting x-only key, `tr(<aggregate>,and_v(v:after(T),and_v(v:older(4259839),pk(<hww>))))`, so standard tools can derive and watch the address without MuSig2 support. `older(4259839)` is BIP68's time-based flag (`0x400000`) plus 65,535. `T` is an absolute Unix-time locktime between 500,000,000 (1985) and 1,700,000,000 (2023), always long past, so it never delays recovery; the recovery transaction just sets its lock time to `T`, which costs 7 witness bytes. Its only job is to choose the address without touching any key: `anzen phone init --vanity` grinds it on the GPU until the address starts with `vault`, and every other vault uses 500,000,000. This is Anzen's only principal-holding script. Monthly allowances and emergency access are implemented with presigned transaction chains using Bitcoin-enforced relative timelocks.

Revocable authorization state lives in separate 1,000-sat connector outputs. They reuse the same device keys under a fixed 1-of-2 Taproot policy:

```text
tr(NUMS,{pk(phone),pk(hww)})
```

Either device can spend a connector alone. The connector does not control vault principal; its exact outpoint is an execution token committed into a presigned policy PSBT.

### Annual vault layout and presigned transaction graph

Once per year, the phone proposes a policy and the HWW approves it once. Together they produce the joint signature on the rollover's vault inputs and on the vault input of each future action. Only the rollover is broadcast immediately. After it confirms, the vault has one allowance-chain UTXO, one remainder UTXO, and one small connector for each enabled independent action chain. Future action PSBTs remain incomplete and encrypted on the phone until it signs the current connector at execution.

![The sequential allowance chain and emergency transactions for an example 2.1 BTC annual vault policy](media/vault-utxo-layout.svg)

The overview shows the state immediately after rollover. Large blue circles are principal-holding vault UTXOs, small amber circles are the controller-wallet connectors, and boxes are transactions. A dashed outline means that output or transaction has not happened yet. Round principal amounts are shown so the policy is easy to read; the checked-in test vector contains exact fees and values.

#### Monthly allowance

![Twelve sequential monthly allowances with a connector rolling alongside the vault state, plus dynamic revocation](media/monthly-allowance-chain.svg)

Each monthly transaction has exactly two inputs: the current allowance vault UTXO and the current connector. After roughly 30 days, the phone signs the connector input and releases 0.1 BTC. Except for month twelve, that transaction creates both the next smaller vault UTXO and the exact connector required by the next step. Spending the live connector in a dynamic revocation leaves the current vault UTXO untouched and makes the current and every dependent future allowance impossible.

#### Emergency access

![Emergency access trigger, delayed withdrawal, and connector-only cancellation](media/emergency-access-chain.svg)

The immediate trigger spends the vault remainder and emergency connector, then creates a staged 0.5 BTC vault output, cold change, and the withdrawal connector. After one week, the staged output and connector can be spent together to the hot wallet. Before then, either device can cancel by spending only the withdrawal connector; both principal outputs remain under the vault script and the delayed withdrawal becomes permanently invalid.

Connector change never returns to the controller address: phone revocation sends it to a fresh hot-wallet change address, while HWW revocation requires an explicitly displayed destination. A later annual rollover spends every live vault and connector output, restarts the hardware wallet's delay, creates only the renewed policy's connectors, and invalidates the old epoch.

Presigned transactions are convenience permissions, not custody. Losing them cannot lose the bitcoin because every principal output still has both vault-script paths above.

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

For Mutinynet, pass `--network mutinynet` when initializing the first device (`anzen --network mutinynet hww init`); the other device, the vault and every later command follow it, and an existing vault cannot be switched to another network. Mutinynet uses its public Electrum WebSocket servers by default; use `--chain-backend rpc --rpc-url …` for your own Mutinynet node.

For a vault address beginning `vault` (`bc1pvault`, `tb1pvault` or `bcrt1pvault`), initialize the HWW first and run `anzen phone init --vanity`. It grinds phone keys on the GPU: about 33 million candidates on average, a minute or two on a desktop. Pass `--cpu` to grind on every CPU core with the library code instead, a few minutes on a desktop. `anzen phone rotate-key --vanity` keeps the prefix when rotating.

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
Cold storage descriptor: tr(996786536ee50cc69bb71143d53d452457c16fcb89d92ddeea070e65b611d37f,and_v(v:after(500000000),and_v(v:older(4259839),pk(1f6ac05a5a5b6ef31c8ab20fa692e327b43e25532d03853f5563108c6d9da33c))))#dken297g
Vault address: bcrt1pwayhsfgr9ptwgw8w3lzphtzxnrq8r3fcuw6dzmacmnmt3p5692nq7a6u07
HWW alone:      after 33,553,920 seconds (~388 days); renewing the vault restarts it
Monthly spending: disabled
Emergency access: disabled
Cloud recovery backup: phone key + descriptor encrypted to the HWW

$ anzen policy
Cold storage descriptor: tr(996786536ee50cc69bb71143d53d452457c16fcb89d92ddeea070e65b611d37f,and_v(v:after(500000000),and_v(v:older(4259839),pk(1f6ac05a5a5b6ef31c8ab20fa692e327b43e25532d03853f5563108c6d9da33c))))#dken297g
Vault address: bcrt1pwayhsfgr9ptwgw8w3lzphtzxnrq8r3fcuw6dzmacmnmt3p5692nq7a6u07
Policy controller address: bcrt1pxu6dxg6pmryuqtuu39ukf56d5ydqa72gv05z23cyyxc65q9cdn3sxe46p7
Policy controller reserve: 1000 sats per active chain
HWW alone:      after 33,553,920 seconds (~388 days); renewing the vault restarts it
Monthly spending: disabled
Emergency access: disabled
```

The new vault starts with monthly spending and emergency access disabled. `anzen init` prints the static cold-storage descriptor (with the two keys' MuSig2 aggregate as its internal key), vault address, and the hardware wallet's delay, and encrypts the phone key and descriptor into the cloud backup with its key wrapped for the HWW. It does not create or sign a spending policy.

### Set or replace the vault policy

The phone proposes the policy, including a fresh MuSig2 nonce for each joint signature. The HWW independently reconstructs the graph, asks for one approval, and returns its half of every joint signature the policy implies. The phone then checks those halves, completes the joint signatures, and broadcasts the rollover. Future controller inputs stay unsigned until the phone executes an action, so neither device commits revocation authority into the annual package. This real regtest policy combines a 0.1 BTC monthly limit with one cancellable 0.5 BTC emergency withdrawal:

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
Rollover fee: 240 sats
Initial allowance-chain UTXO: 120002274 sats
Rollover remainder: 79995486 sats
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
Initial allowance-chain UTXO: 120002274 sats
Rollover remainder: 79995486 sats
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
Vault balance: 199997760 sats
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
New cold storage descriptor: tr(b96981905428d81ee3b0e1c1bee9030862d51d57d391da05f22b560b1906ca7a,and_v(v:after(500000000),and_v(v:older(4259839),pk(39e34b45151097baa5bf11460423f84104cc48c033abc95c20d7ce8c392c16df))))#4m8xy3u8
New vault address: bcrt1ps7ntxatqny2l02neaymtezw5r9uny8tgy9u2v6quav0u8ef8qy0qz2mp44
Inputs: 2
Sent: 199997591 sats
Fee: 169 sats (1 sat/vB)
Old policy controllers revoked: 2
Monthly policy preserved: 10000000 sats
Renewed allowance steps: 12
Renewed policy transactions: 15
Emergency access preserved: 50000000 sats
Phone-key rotation proposal: phone-rotation.json

$ anzen hww confirm-rotation phone-rotation.json --output approved-phone-rotation.json --yes
PHONE-KEY ROTATION
New phone vault key: 04d7b62732a46111b412e445925ecab69570c1c58a036459044d3dd8fc42c794
New cold storage descriptor: tr(b96981905428d81ee3b0e1c1bee9030862d51d57d391da05f22b560b1906ca7a,and_v(v:after(500000000),and_v(v:older(4259839),pk(39e34b45151097baa5bf11460423f84104cc48c033abc95c20d7ce8c392c16df))))#4m8xy3u8
New vault address: bcrt1ps7ntxatqny2l02neaymtezw5r9uny8tgy9u2v6quav0u8ef8qy0qz2mp44
Inputs: 2
Sent: 199997591 sats
Fee: 169 sats (1 sat/vB)
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
New vault address: bcrt1ps7ntxatqny2l02neaymtezw5r9uny8tgy9u2v6quav0u8ef8qy0qz2mp44
Phone mnemonic: room absurd inject pole depth exit tenant wisdom cook predict net practice dry clarify panther album gossip million else maximum aim use they buyer
Monthly policy preserved: 10000000 sats
Policy rollover broadcast: 996bef84a3d995d2f791764f7f6d88987d4f86e8fe371c33996639e3b33cb7ad
Encrypted allowance authorizations: 12
Emergency access preserved: 50000000 sats

$ anzen hww adopt-rotation approved-phone-rotation.json
HWW paired with the rotated vault: bcrt1ps7ntxatqny2l02neaymtezw5r9uny8tgy9u2v6quav0u8ef8qy0qz2mp44
```

The rotation preserves the HWW key and the active monthly and emergency-access limits. Before installing the new phone key it spends every old controller to replacement-phone change, so no old-key policy authority survives. It then sweeps the old vault cooperatively to a descriptor built from the replacement phone key and writes a new descriptor-bound cloud backup. The same proposal chains a fresh annual policy to that sweep; one HWW prompt approves both, and all replacement artifacts and connectors use the new phone key. A disabled feature remains disabled after rotation.

### Lose the phone and its backup

The cloud backup payload contains the phone mnemonic and cold-storage descriptor, authenticated-encrypted under one random symmetric key that only the HWW can unwrap. There is no other recipient: losing both devices is final, even if the encrypted cloud backup survives, because only the HWW can release it and only the HWW's key can sign the delayed path.

If the phone and its backup are permanently unavailable, the HWW alone recovers the savings once a year has passed since the last renewal. Initialize a replacement vault in a separate data directory (`phone init`, `hww init`, and `init` with `--data-dir .replacement-vault`) and take a receive address from its phone. Before the delay, the HWW refuses to sign a sweep Bitcoin would reject; once the chain's median time past reaches the delay, the same command recovers directly to that address. These outputs are from the lost-phone-no-cloud test's own vault:

```console
$ anzen --data-dir .replacement-vault phone receive-address
Hot receive address: bcrt1p942dzxwt78787mqjl40t4wym6qnz42rculqrerh59flwdtflm2es57ksr4

$ anzen hww recover bcrt1p942dzxwt78787mqjl40t4wym6qnz42rculqrerh59flwdtflm2es57ksr4
HWW recovery sweep broadcast: c6cbf842d5e6e0489a377fdd1c56efc7ceff4f7d389720daba9d7235017c0548
Inputs: 1
Sent: 199999870 sats
Fee: 130 sats (1 sat/vB)
```

This delayed recovery moves the funds to the replacement wallet, whose phone and HWW keys are both new. `anzen status` shows when the HWW's path opens for the oldest vault output.

### Lose the HWW

Restore the HWW from the backup its maker provides. It comes back as the same key, so the vault, policy, and phone are unchanged and nothing needs to move. Until then, the phone keeps using the year's monthly allowances and emergency access; it can never move the savings alone, so a stolen phone gains no more than that bounded budget. If the HWW and its maker's backup are both gone for good, nothing can move the savings; the phone keeps only the allowances already approved. Keep the HWW backup as carefully as the device.

### Other cooperative sweeps

Arbitrary immediate vault sweeps retain the same explicit device boundary. The phone proposes the sweep with a fresh MuSig2 nonce per input, without signing it:

```console
$ anzen phone create-sweep bcrt1pmzwluktvjp6mvktghy7yp8pyervxzxjnzxauqdnuwvzfjjcy449svdaupv --output sweep.json
COOPERATIVE VAULT SWEEP
Destination: bcrt1pmzwluktvjp6mvktghy7yp8pyervxzxjnzxauqdnuwvzfjjcy449svdaupv
Inputs: 1
Sent: 199999889 sats
Fee: 111 sats (1 sat/vB)
Cooperative sweep proposal: sweep.json
```

The HWW then reviews and signs it with `anzen hww confirm-sweep sweep.json --output approved-sweep.json`, and `anzen phone broadcast-sweep approved-sweep.json` checks the HWW's partial signatures, completes the joint signatures, and broadcasts. This proposal comes from the stolen-phone test, where the HWW is absent, so `confirm-sweep` fails and the vault stays put; the end-to-end tests do not show a completed cooperative sweep.

## Run the end-to-end tests

Docker is the only host dependency:

```bash
./scripts/run-e2e.sh --list
./scripts/run-e2e.sh monthly-spend
./scripts/run-e2e.sh lost-phone lost-hww
./scripts/run-e2e.sh all
```

With no arguments, the runner behaves like `all`. Selected tests run serially, and each gets a fresh regtest chain and vault state so it can be read and reproduced independently. Separate runner invocations also use isolated Compose projects, so concurrent local tests cannot stop or erase one another. Output is limited to user actions, the corresponding CLI commands, essential policy/transaction results, expected safety rejections, and compact chain progress. Displayed commands omit the internal `--data-dir` argument, retain the terminal's default color, and show their results in muted grey. Every completed step starts a new paragraph with a short `✅` outcome so the test can be understood by skimming those lines.

The named tests cover setup/policy, sequential monthly spend, whole-chain monthly revoke, successful and cancelled one-week emergency access, partial funding, lost or stolen phone, lost or stolen HWW, missing cloud backup, both devices lost, cloud compromise, both keys compromised, and both on-time and forgotten annual rollover. The spend demonstrations fund exactly 2 BTC and create one chain funding up to twelve 0.1 BTC allowance releases.

Recovery tests advance the node's mock time past the real ~388-day HWW delay instead of mining it: eleven blocks stamped with a later time move the median time past that BIP68 checks. Every long-delay test still proves its behavior on an independent chain.

## Run all tests

```bash
./scripts/run-tests.sh
```

This runs unit and CLI tests and the real-node integration tests, including the recovery test that checks Bitcoin Core rejects the HWW's sweep one second before its delay and accepts it on time. For fast local development without Docker:

```bash
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
./scripts/check-boundaries.sh
```

Project layout, development commands, and the hardware-firmware workflow are documented in [DEVELOPMENT.md](DEVELOPMENT.md). Implementation rationale and engineering trade-offs are recorded in [design-decisions.md](design-decisions.md).

## License

[MIT](LICENSE) © 2026 Luke Childs
