//! Anzen reference CLI: two simulated devices sharing one directory for demonstration.
//!
//! `phone` commands use only the wallet application and `hww` commands only the signer
//! application. Messages between them travel as files, exactly as they would between physical
//! devices. Top-level commands such as `init` stand in for the user operating both devices.

use anyhow::{Context, Result, bail};
use anzen_protocol::{address, crypto::SoftwareCrypto, script::Scripts};
use anzen_signer_app::{self as cold_wallet, application::SignerApplication};
use anzen_wallet::{
    self as hot_wallet, HotWalletBackend,
    api::{
        ApprovedDevicePhoneRotation, CloudRecoveryBackup, DeviceMessage, DevicePhoneRotation,
        OfflineAction, PolicyRequest, PublicDeviceFile, RecoveryVaultBundle, RestoreKeyRequest,
        SweepApproval, SweepRequest, TransactionKind, VaultConfig,
    },
    application::WalletApplication,
};
use bitcoin::Network;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(name = "anzen", version, about)]
struct Cli {
    /// Directory containing simulated device, cloud, and wallet state.
    #[arg(long, default_value = ".anzen-data", global = true)]
    data_dir: PathBuf,

    /// Required on initialization and every later command for a mainnet vault.
    #[arg(long, global = true)]
    dangerously_enable_mainnet: bool,

    /// Test network for new devices. Existing devices and vaults keep the network they were
    /// created on.
    #[arg(long, env = "ANZEN_NETWORK", value_enum, global = true)]
    network: Option<TestNetwork>,

    #[command(flatten)]
    chain: ChainArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Pair the initialized devices, compare the vault, and store the first cloud backup.
    Init,
    /// Print the configured cold-storage and active programmable policy.
    Policy,
    /// Print vault and chain balances from the configured chain backend.
    Status,
    /// Actions performed by the simulated phone.
    Phone {
        #[command(subcommand)]
        command: PhoneCommand,
    },
    /// Actions performed by the simulated hardware wallet.
    Hww {
        #[command(subcommand)]
        command: HwwCommand,
    },
    /// Regtest-only node controls used by the end-to-end tests.
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
}

#[derive(Debug, Subcommand)]
enum PhoneCommand {
    /// Create the simulated phone key and hot-wallet account.
    Init {
        /// Grind a combined HWW/phone vault address beginning with bc1pvault, tb1pvault or
        /// bcrt1pvault.
        #[arg(long)]
        vanity: bool,
        /// Grind on every CPU thread with the library code instead of the GPU.
        #[arg(long, requires = "vanity")]
        cpu: bool,
    },
    /// Derive and persist the next hot-wallet receive address.
    ReceiveAddress,
    /// Send an exact amount from the phone hot wallet.
    Send { address: String, amount_sats: u64 },
    /// Propose a vault policy for the HWW to review.
    SetPolicy {
        #[arg(long)]
        monthly_limit: u64,
        /// One delayed emergency withdrawal available during this vault epoch; zero disables it.
        #[arg(long, default_value_t = 0)]
        emergency_access_limit: u64,
        #[arg(long)]
        output: PathBuf,
        /// Accepted for compatibility with deterministic regtest scripts.
        #[arg(long, hide = true)]
        now: Option<i64>,
    },
    /// Verify an HWW-approved policy, add phone signatures, broadcast its rollover, and store it.
    ActivatePolicy { approved_policy: PathBuf },
    /// Broadcast the next presigned allowance authorization after its relative delay.
    Authorize { step: u8 },
    /// Return the difference between an authorization and a lower soft limit.
    ApplySoftLimit {
        step: u8,
        #[arg(long)]
        limit: u64,
    },
    /// Revoke this allowance step and every later step in its chain.
    Revoke { step: u8 },
    /// Execute or cancel the epoch's presigned emergency-access package.
    Emergency {
        #[command(subcommand)]
        command: EmergencyCommand,
    },
    /// Import the vault to recover from: an HWW recovery bundle or a public vault configuration.
    ImportRecovery { recovery: PathBuf },
    /// Ask the HWW to release the cloud backup's key to this fresh phone.
    RestoreRequest {
        backup: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Decrypt the cloud backup with the key the HWW released to this phone.
    Restore { response: PathBuf },
    /// Propose a cooperative sweep for the HWW to review.
    CreateSweep {
        destination: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify, sign, and broadcast an HWW-approved cooperative sweep.
    BroadcastSweep { approved_sweep: PathBuf },
    /// Propose a new phone key while preserving the active vault policy.
    RotateKey {
        /// Grind until the new vault address begins with bc1pvault, tb1pvault or bcrt1pvault.
        #[arg(long)]
        vanity: bool,
        /// Grind on every CPU thread with the library code instead of the GPU.
        #[arg(long, requires = "vanity")]
        cpu: bool,
        #[arg(long)]
        output: PathBuf,
    },
    /// Activate an HWW-approved key rotation and replacement annual policy.
    ActivateRotation { approved_rotation: PathBuf },
    /// Store the cloud backup the HWW approved.
    StoreBackup { response: PathBuf },
}

#[derive(Debug, Subcommand)]
enum EmergencyCommand {
    /// Start the one-week emergency-access cancellation window.
    Initiate,
    /// Withdraw the configured amount after the one-week delay.
    Withdraw,
    /// Cancel an initiated emergency withdrawal and return it to the vault.
    Cancel,
}

#[derive(Debug, Subcommand)]
enum HwwCommand {
    /// Create the simulated HWW key.
    Init,
    /// Review one high-level policy and sign every transaction it implies.
    ConfirmPolicy {
        proposal: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Approve non-interactively; intended for deterministic tests.
        #[arg(long)]
        yes: bool,
    },
    /// Forget the last approved policy after confirming it was never activated.
    ForgetPolicy {
        #[arg(long)]
        yes: bool,
    },
    /// Review a new cloud backup and wrap its key for this HWW.
    ConfirmBackup {
        request: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Export the vault and last signed policy for a replacement phone.
    ExportRecovery {
        #[arg(long)]
        output: PathBuf,
    },
    /// Release a cloud backup's key to a fresh replacement phone.
    ApproveRestore {
        request: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Sweep vault outputs past the signer's time lock (about 388 days) with the HWW alone.
    Recover { destination: String },
    /// Immediately invalidate every live programmable-policy state.
    RevokePolicy {
        /// Verified safe destination for the small controller-output remainder.
        destination: String,
        #[arg(long)]
        yes: bool,
    },
    /// Review and sign a phone-proposed cooperative sweep.
    ConfirmSweep {
        proposal: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Approve a phone-key rotation and any replacement policy in one prompt.
    ConfirmRotation {
        proposal: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Pair with the rotated vault after the phone broadcast the approved rotation.
    AdoptRotation { approved_rotation: PathBuf },
}

#[derive(Debug, Clone, Args)]
struct ChainArgs {
    /// Chain backend to use. Defaults to RPC on regtest and Electrum on mainnet.
    #[arg(long, env = "ANZEN_CHAIN_BACKEND", value_enum, global = true)]
    chain_backend: Option<ChainBackendKind>,
    #[arg(long, env = "ANZEN_RPC_URL", global = true)]
    rpc_url: Option<String>,
    #[arg(long, env = "ANZEN_RPC_USER", default_value = "anzen", global = true)]
    rpc_user: String,
    #[arg(
        long,
        env = "ANZEN_RPC_PASSWORD",
        default_value = "anzen",
        global = true
    )]
    rpc_password: String,
    /// Use one Electrum WebSocket endpoint (wss://) instead of the network's built-in default.
    #[arg(long, env = "ANZEN_ELECTRUM_URL", global = true)]
    electrum_url: Option<String>,
}

/// Networks that hold no real bitcoin. Mainnet is opted into separately, on every command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TestNetwork {
    /// A local Bitcoin Core regtest node (the default).
    Regtest,
    /// The public Mutinynet signet, over its Electrum WebSocket servers.
    Mutinynet,
}

impl TestNetwork {
    fn bitcoin(self) -> Network {
        match self {
            Self::Regtest => Network::Regtest,
            Self::Mutinynet => Network::Signet,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ChainBackendKind {
    Rpc,
    Electrum,
}

impl ChainArgs {
    fn selected_backend(&self, network: Network) -> Result<ChainBackendKind> {
        self.chain_backend.map_or_else(
            || match network {
                Network::Regtest => Ok(ChainBackendKind::Rpc),
                Network::Bitcoin | Network::Signet => Ok(ChainBackendKind::Electrum),
                other => bail!("unsupported vault network: {other}"),
            },
            Ok,
        )
    }

    fn connect_core(&self, network: Network) -> Result<anzen_chain::BitcoinCoreBackend> {
        let default_url = match network {
            Network::Regtest => "http://127.0.0.1:18443",
            Network::Bitcoin => "http://127.0.0.1:8332",
            // Mutinynet nodes run a custom signet with no standard local port.
            Network::Signet if self.rpc_url.is_none() => {
                bail!("pass --rpc-url to use a Mutinynet Bitcoin Core node")
            }
            Network::Signet => "",
            other => bail!("unsupported Bitcoin Core network: {other}"),
        };
        anzen_chain::BitcoinCoreBackend::connect(
            &anzen_chain::RpcConfig {
                url: self
                    .rpc_url
                    .clone()
                    .unwrap_or_else(|| default_url.to_owned()),
                user: self.rpc_user.clone(),
                password: self.rpc_password.clone(),
            },
            network,
        )
    }

    fn connect_electrum(
        &self,
        network: Network,
    ) -> Result<hot_wallet::electrum::ElectrumBackend<hot_wallet::api::WebSocketConnector>> {
        use hot_wallet::{api::WebSocketConnector, electrum::ElectrumBackend};
        match &self.electrum_url {
            Some(server) => {
                ElectrumBackend::with_servers(network, vec![server.clone()], WebSocketConnector)
            }
            None => ElectrumBackend::new(network, WebSocketConnector),
        }
    }

    /// A public chain view for a device that holds no wallet of its own.
    fn connect_chain(&self, network: Network) -> Result<Box<dyn anzen_chain::Blockchain>> {
        match self.selected_backend(network)? {
            ChainBackendKind::Rpc => Ok(Box::new(self.connect_core(network)?)),
            ChainBackendKind::Electrum => Ok(Box::new(self.connect_electrum(network)?)),
        }
    }

    fn connect_hot(&self, network: Network) -> Result<Box<dyn HotWalletBackend>> {
        match self.selected_backend(network)? {
            ChainBackendKind::Rpc => Ok(Box::new(self.connect_core(network)?)),
            ChainBackendKind::Electrum => Ok(Box::new(self.connect_electrum(network)?)),
        }
    }
}

#[derive(Debug, Subcommand)]
enum NodeCommand {
    /// Print the current regtest height and median-time-past.
    Info,
    /// Set the regtest node's mock Unix time.
    SetTime { timestamp: u64 },
    /// Mine blocks immediately to an explicit regtest address.
    Mine { blocks: u64, address: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let network = command_network(&cli.data_dir, cli.dangerously_enable_mainnet, cli.network)?;
    let allow_mainnet = network == Network::Bitcoin;
    match cli.command {
        Command::Init => initialize_vault(&cli.data_dir, network, allow_mainnet),
        Command::Policy => print_active_policy(&wallet(&cli.data_dir, allow_mainnet)),
        Command::Status => print_status(
            &cli.data_dir,
            &wallet(&cli.data_dir, allow_mainnet),
            &signer(&cli.data_dir, allow_mainnet),
            &cli.chain,
        ),
        Command::Phone { command } => run_phone(
            command,
            &wallet(&cli.data_dir, allow_mainnet),
            &cli.chain,
            network,
        ),
        Command::Hww { command } => run_hww(
            command,
            &cli.data_dir,
            &signer(&cli.data_dir, allow_mainnet),
            &cli.chain,
            network,
        ),
        Command::Node { command } => run_node(command, &cli.chain, network),
    }
}

fn wallet(data_dir: &Path, allow_mainnet: bool) -> WalletApplication {
    WalletApplication::new(data_dir.to_path_buf(), allow_mainnet)
}

fn signer(data_dir: &Path, allow_mainnet: bool) -> SignerApplication {
    SignerApplication::new(data_dir.to_path_buf(), allow_mainnet)
}

/// The user operating both simulated devices: exchange public keys, compare the vault on both
/// screens, then store the first cloud backup whose key only the HWW can release.
fn initialize_vault(data_dir: &Path, network: Network, allow_mainnet: bool) -> Result<()> {
    let wallet = wallet(data_dir, allow_mainnet);
    let signer = signer(data_dir, allow_mainnet);
    let phone = wallet
        .public_identity()
        .context("initialize the phone before the vault")?;
    let hww = signer
        .public_identity()
        .context("initialize the HWW before the vault")?;
    if phone.bitcoin_network()? != network || hww.bitcoin_network()? != network {
        bail!("phone, HWW, and vault must be initialized for the same network");
    }
    let config = wallet.pair_signer(&hww)?;
    signer.accept_vault(&config)?;
    let request = wallet.propose_backup()?;
    let review = signer.review_backup(&request)?;
    wallet.store_backup(&signer.approve_backup(&review.review_id, &request)?)?;
    wallet.export_backup()?;
    println!("Vault initialized ({})", network_label(network));
    if network == Network::Bitcoin {
        println!("DANGER: mainnet mode uses real bitcoin and fixed 1 sat/vB MVP fees");
    }
    println!("Cold storage descriptor: {}", config.vault_descriptor);
    println!("Vault address: {}", config.vault_address);
    print_recovery_delay(&config);
    println!("Monthly spending: disabled");
    println!("Emergency access: disabled");
    println!("Cloud recovery backup: phone key + descriptor encrypted to the HWW");
    Ok(())
}

fn run_phone(
    command: PhoneCommand,
    wallet: &WalletApplication,
    rpc_args: &ChainArgs,
    network: Network,
) -> Result<()> {
    let backend = || rpc_args.connect_hot(network);
    match command {
        PhoneCommand::Init { vanity, cpu } => {
            if vanity {
                let target = hot_wallet::vanity_address_prefix(network)?;
                let engine = vanity_engine(cpu);
                eprintln!(
                    "Grinding phone vault keys {} for {target}...",
                    engine_label(engine)
                );
                let signer: PublicDeviceFile = read_artifact(
                    &wallet_data_dir(wallet).join(anzen_common::storage::HWW_PUBLIC_FILE),
                )
                .context("initialize the HWW before using phone init --vanity")?;
                let result = wallet
                    .initialize_vanity(&signer, engine, report_vanity_progress)
                    .map_err(|error| gpu_hint(error, engine))?;
                println!("Simulated phone initialized ({})", network_label(network));
                println!("Phone mnemonic: {}", result.device.mnemonic);
                println!("Phone vault key: {}", result.device.vault_pubkey);
                println!("Phone vault key index: {}", result.device.vault_key_index);
                println!("Vanity vault address: {}", result.vault_address);
                println!(
                    "Vanity search: {} candidates on {}",
                    format_number(result.attempts),
                    result.searcher
                );
                return Ok(());
            }
            let phone = hot_wallet::initialize(&wallet_data_dir(wallet), network)?;
            println!("Simulated phone initialized ({})", network_label(network));
            println!("Phone mnemonic: {}", phone.mnemonic);
            println!("Phone vault key: {}", phone.vault_pubkey);
        }
        PhoneCommand::ReceiveAddress => {
            println!("Hot receive address: {}", wallet.receive_address()?);
        }
        PhoneCommand::Send {
            address,
            amount_sats,
        } => {
            let backend = backend()?;
            let review = wallet.review_payment(backend.as_ref(), &address, amount_sats)?;
            let txid = wallet.confirm_payment(backend.as_ref(), &review.review_id)?;
            println!("Hot-wallet payment broadcast: {txid}");
            println!("Destination: {}", review.destination);
            println!("Amount: {} sats", review.amount_sats);
            println!("Fee: {} sats (1 sat/vB)", review.fee_sats);
        }
        PhoneCommand::SetPolicy {
            monthly_limit,
            emergency_access_limit,
            output,
            now,
        } => {
            if now.is_some() && network != Network::Regtest {
                bail!("--now is available only for deterministic regtest tests");
            }
            let request = wallet.propose_policy(
                backend()?.as_ref(),
                hot_wallet::api::PolicyLimits {
                    monthly_limit_sats: monthly_limit,
                    emergency_access_limit_sats: emergency_access_limit,
                },
            )?;
            print_policy_request(&request, artifact_reports_to_stderr(&output))?;
            write_artifact(&output, &request)?;
            report_artifact(&output, "Policy proposal")?;
        }
        PhoneCommand::ActivatePolicy { approved_policy } => {
            let approval = hot_wallet::open_approved_policy(
                &wallet_data_dir(wallet),
                read_artifact(&approved_policy)?,
            )?;
            let schedule = wallet.activate_policy(backend()?.as_ref(), &approval)?;
            println!("Rollover broadcast: {}", schedule.rollover_txid);
            println!("Active monthly limit: {} sats", schedule.monthly_limit_sats);
            println!(
                "Encrypted allowance authorizations: {}",
                schedule.entries.len()
            );
            match &schedule.emergency_access {
                Some(emergency) => {
                    println!("Active emergency access: {} sats", emergency.amount_sats);
                    println!("Encrypted emergency transaction set: trigger, withdrawal");
                }
                None => println!("Emergency access: disabled"),
            }
        }
        PhoneCommand::Authorize { step } => {
            let txid = wallet.monthly(backend()?.as_ref(), step, TransactionKind::Authorization)?;
            println!("Broadcast Authorization for allowance step {step}: {txid}");
        }
        PhoneCommand::ApplySoftLimit { step, limit } => {
            match wallet.apply_soft_limit(backend()?.as_ref(), step, limit)? {
                Some(txid) => println!(
                    "Soft limit applied for allowance step {step}: retained at most {limit} sats hot; cold-return txid={txid}"
                ),
                None => println!(
                    "Soft limit equals the monthly limit for allowance step {step}; no cold-return transaction required"
                ),
            }
        }
        PhoneCommand::Revoke { step } => {
            let txid = wallet.monthly(backend()?.as_ref(), step, TransactionKind::Revocation)?;
            println!("Broadcast Revocation for allowance step {step}: {txid}");
        }
        PhoneCommand::Emergency { command } => {
            let backend = backend()?;
            match command {
                EmergencyCommand::Initiate => {
                    let txid = wallet.initiate_emergency(backend.as_ref())?;
                    println!("Emergency access initiated: {txid}");
                    let emergency = wallet
                        .vault_snapshot(backend.as_ref())?
                        .schedule
                        .and_then(|schedule| schedule.emergency_access)
                        .context("active schedule omitted emergency access metadata")?;
                    println!("Amount after delay: {} sats", emergency.amount_sats);
                    println!("Cancellation window: {} seconds", emergency.delay_seconds);
                }
                EmergencyCommand::Withdraw => {
                    let txid = wallet.withdraw_emergency(backend.as_ref())?;
                    println!("Emergency access withdrawal broadcast: {txid}");
                }
                EmergencyCommand::Cancel => {
                    let txid = wallet.cancel_emergency(backend.as_ref())?;
                    println!("Emergency access cancelled: {txid}");
                }
            }
        }
        PhoneCommand::ImportRecovery { recovery } => {
            let config = match read_artifact::<RecoverySource>(&recovery)? {
                RecoverySource::Bundle(bundle) => {
                    wallet.import_recovery_bundle(&bundle)?;
                    bundle.config
                }
                RecoverySource::Config(config) => {
                    wallet.import_recovery_vault(&config)?;
                    *config
                }
            };
            println!("Recovery vault imported: {}", config.vault_address);
        }
        PhoneCommand::RestoreRequest { backup, output } => {
            let backup: CloudRecoveryBackup = read_artifact(&backup)?;
            let request = wallet.propose_restore(&backup)?;
            println!(
                "Restore request for vault {}",
                wallet.recovery_source()?.vault_address
            );
            println!("Replacement phone key: {}", request.recipient.vault_pubkey);
            write_artifact(&output, &request)?;
            report_artifact(&output, "Restore request")?;
        }
        PhoneCommand::Restore { response } => {
            let message: DeviceMessage = read_artifact(&response)?;
            let restored = wallet.store_restored_key(&message)?;
            println!("Phone key restored from the authenticated cloud backup");
            println!("Recovered phone key: {}", restored.old_phone_public_key);
            println!("Replacement phone key: {}", restored.new_phone_public_key);
            println!("Rotate the vault to the replacement key before using it");
        }
        PhoneCommand::CreateSweep {
            destination,
            output,
        } => {
            let request = wallet.propose_sweep(backend()?.as_ref(), &destination)?;
            report_sweep(&request, artifact_reports_to_stderr(&output))?;
            write_artifact(&output, &request)?;
            report_artifact(&output, "Cooperative sweep proposal")?;
        }
        PhoneCommand::BroadcastSweep { approved_sweep } => {
            let approval: SweepApproval = read_artifact(&approved_sweep)?;
            let result = wallet.activate_sweep(backend()?.as_ref(), &approval)?;
            print_sweep_result("Cooperative vault sweep broadcast", &result);
        }
        PhoneCommand::RotateKey {
            vanity,
            cpu,
            output,
        } => {
            let backend = backend()?;
            let request = if !anzen_common::storage::config_exists(&wallet_data_dir(wallet))
                && wallet.restored_phone().is_ok()
            {
                if vanity {
                    bail!("a restored phone rotates to its own new key; --vanity is unavailable");
                }
                wallet.propose_restored_rotation(backend.as_ref())?
            } else {
                let engine = vanity.then(|| vanity_engine(cpu));
                if let Some(engine) = engine {
                    eprintln!(
                        "Preparing a vanity phone-key rotation for {} {}; this may take a while...",
                        hot_wallet::vanity_address_prefix(network)?,
                        engine_label(engine)
                    );
                }
                wallet
                    .propose_rotation(backend.as_ref(), engine, report_vanity_progress)
                    .map_err(|error| match engine {
                        Some(engine) => gpu_hint(error, engine),
                        None => error,
                    })?
            };
            report_rotation(&request, artifact_reports_to_stderr(&output))?;
            write_artifact(&output, &request)?;
            report_artifact(&output, "Phone-key rotation proposal")?;
        }
        PhoneCommand::ActivateRotation { approved_rotation } => {
            let approval: ApprovedDevicePhoneRotation = read_artifact(&approved_rotation)?;
            let result = wallet.activate_rotation(backend()?.as_ref(), &approval)?;
            println!("Phone-key rotation sweep broadcast: {}", result.sweep_txid);
            if let Some(txid) = result.controller_revocation_txid {
                println!("Old policy controllers revoked: {txid}");
            }
            if let Some(txid) = result.hot_wallet_sweep_txid {
                println!("Old hot-wallet funds swept to replacement phone: {txid}");
            }
            println!("Old vault address: {}", result.old_vault_address);
            println!("New vault address: {}", result.new_vault_address);
            let device = anzen_common::storage::load_device(
                &wallet_data_dir(wallet),
                anzen_common::storage::PHONE_DEVICE_FILE,
            )?;
            println!("Phone mnemonic: {}", device.mnemonic);
            match result.renewed_schedule {
                Some(schedule) => {
                    println!(
                        "Monthly policy preserved: {} sats",
                        schedule.monthly_limit_sats
                    );
                    println!("Policy rollover broadcast: {}", schedule.rollover_txid);
                    println!(
                        "Encrypted allowance authorizations: {}",
                        schedule.entries.len()
                    );
                    if let Some(emergency) = &schedule.emergency_access {
                        println!("Emergency access preserved: {} sats", emergency.amount_sats);
                    }
                }
                None => println!("Programmable vault policy remains disabled"),
            }
        }
        PhoneCommand::StoreBackup { response } => {
            let message: DeviceMessage = read_artifact(&response)?;
            wallet.store_backup(&message)?;
            println!("Cloud recovery backup stored");
        }
    }
    Ok(())
}

/// A vault to recover from, as exported by an HWW or copied from the public configuration.
#[derive(Deserialize)]
#[serde(untagged)]
enum RecoverySource {
    Bundle(Box<RecoveryVaultBundle>),
    Config(Box<VaultConfig>),
}

fn run_hww(
    command: HwwCommand,
    data_dir: &Path,
    signer: &SignerApplication,
    rpc_args: &ChainArgs,
    network: Network,
) -> Result<()> {
    match command {
        HwwCommand::Init => {
            let hww = cold_wallet::initialize(data_dir, network)?;
            println!("Simulated HWW initialized ({})", network_label(network));
            println!("HWW mnemonic: {}", hww.mnemonic);
            println!("HWW vault key: {}", hww.vault_pubkey);
            println!(
                "HWW ready to pair with the phone and wrap its cloud backup key at anzen init"
            );
        }
        HwwCommand::ConfirmPolicy {
            proposal,
            output,
            yes,
        } => {
            let request: PolicyRequest = read_artifact(&proposal)?;
            let review = signer.review_policy(&request)?;
            eprintln!("SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL");
            print_policy_request(&request, true)?;
            if let Some(warning) = &review.previous_policy_warning {
                eprintln!("WARNING: {warning}");
            }
            require_hww_approval(yes, &proposal, "complete vault policy")?;
            let approval = signer.approve_policy(&review.review_id)?;
            eprintln!(
                "HWW validated and signed all {} presigned transactions after one approval",
                review.transaction_count
            );
            write_artifact(&output, &approval)?;
            report_artifact(&output, "HWW-approved policy")?;
        }
        HwwCommand::ForgetPolicy { yes } => {
            eprintln!("SIMULATED HWW — FORGET THE LAST APPROVED POLICY");
            eprintln!("Only continue if that policy was never activated on the phone");
            require_hww_approval(yes, Path::new("forget-policy"), "policy reset")?;
            signer.forget_approved_policy()?;
            println!("Last approved policy forgotten");
        }
        HwwCommand::ConfirmBackup {
            request,
            output,
            yes,
        } => {
            let message: DeviceMessage = read_artifact(&request)?;
            let review = signer.review_backup(&message)?;
            eprintln!("SIMULATED HWW — CLOUD BACKUP");
            eprintln!("Vault address: {}", review.vault_address);
            eprintln!("Only this HWW can unwrap the phone's cloud backup");
            require_hww_approval(yes, &request, "cloud backup")?;
            let response = signer.approve_backup(&review.review_id, &message)?;
            println!("HWW wrapped the cloud backup key");
            write_artifact(&output, &response)?;
            report_artifact(&output, "HWW-approved cloud backup")?;
        }
        HwwCommand::ExportRecovery { output } => {
            let bundle = signer.export_recovery_bundle()?;
            println!("Vault address: {}", bundle.config.vault_address);
            write_artifact(&output, &bundle)?;
            report_artifact(&output, "HWW recovery bundle")?;
        }
        HwwCommand::ApproveRestore {
            request,
            output,
            yes,
        } => {
            let restore: RestoreKeyRequest = read_artifact(&request)?;
            let review = signer.review_restore(&restore)?;
            eprintln!("SIMULATED HWW — RESTORE A LOST PHONE");
            eprintln!("Vault address: {}", review.vault_address);
            eprintln!("Replacement phone key: {}", review.recipient_public_key);
            eprintln!("Compare this key with the replacement phone before approving");
            require_hww_approval(yes, &request, "phone restore")?;
            let response = signer.approve_restore(&review.review_id)?;
            println!("HWW released the cloud backup key to the replacement phone");
            write_artifact(&output, &response)?;
            report_artifact(&output, "HWW restore response")?;
        }
        HwwCommand::Recover { destination } => {
            // The connected computer scans and broadcasts; the HWW only reviews and signs.
            let config = signer.export_recovery_bundle()?.config;
            let backend = rpc_args.connect_chain(network)?;
            let tip = backend.chain_tip()?;
            let utxos = backend.scan_vault(&config)?;
            let clock = anzen_wallet::recovery::RecoveryClock::read(&*backend, &tip, &utxos)?;
            let request = anzen_wallet::offline_action::mature_recovery_request(
                &config,
                &utxos,
                &clock,
                &destination,
            )?;
            let review = signer.review_offline_action(&request)?;
            let approval = signer.approve_offline_action(&review.review_id)?;
            let transaction = anzen_wallet::offline_action::finalize(&config, &approval)?;
            let txid = backend.broadcast(&transaction)?;
            if txid != transaction.compute_txid() {
                bail!("chain backend returned an unexpected HWW recovery transaction ID");
            }
            println!("HWW recovery sweep broadcast: {txid}");
            println!("Inputs: {}", review.input_count);
            println!("Sent: {} sats", review.amount_sats);
            println!("Fee: {} sats (1 sat/vB)", review.fee_sats);
        }
        HwwCommand::RevokePolicy { destination, yes } => {
            let config = signer.export_recovery_bundle()?.config;
            let backend = rpc_args.connect_chain(network)?;
            let connectors = backend.scan_connectors(&config)?;
            if connectors.is_empty() {
                bail!("no live policy controller outputs are available to revoke");
            }
            let request = anzen_wallet::offline_action::request(
                &config,
                OfflineAction::RevokeActions,
                &connectors,
                &destination,
            )?;
            let review = signer.review_offline_action(&request)?;
            eprintln!("SIMULATED HWW — REVOKE ACTIVE VAULT POLICY");
            eprintln!("Controller outputs: {}", review.input_count);
            eprintln!("Change destination: {}", review.destination);
            eprintln!("All policy transactions committed to these states will be invalidated");
            require_hww_approval(yes, Path::new("controller-revocation"), "policy revocation")?;
            let approval = signer.approve_offline_action(&review.review_id)?;
            let transaction = anzen_wallet::offline_action::finalize(&config, &approval)?;
            let txid = backend.broadcast(&transaction)?;
            if txid != transaction.compute_txid() {
                bail!("chain backend returned an unexpected policy revocation transaction ID");
            }
            println!("HWW policy revocation broadcast: {txid}");
            println!("Revoked controller outputs: {}", connectors.len());
        }
        HwwCommand::ConfirmSweep {
            proposal,
            output,
            yes,
        } => {
            let request: SweepRequest = read_artifact(&proposal)?;
            let review = signer.review_sweep(&request)?;
            report_sweep(&request, true)?;
            require_hww_approval(yes, &proposal, "cooperative sweep")?;
            let approval = signer.approve_sweep(&review.review_id)?;
            eprintln!("HWW validated and signed the cooperative sweep");
            write_artifact(&output, &approval)?;
            report_artifact(&output, "HWW-approved cooperative sweep")?;
        }
        HwwCommand::ConfirmRotation {
            proposal,
            output,
            yes,
        } => {
            let request: DevicePhoneRotation = read_artifact(&proposal)?;
            let review = signer.review_phone_rotation(&request)?;
            report_rotation(&request, true)?;
            if let Some(warning) = &review.previous_policy_warning {
                eprintln!("WARNING: {warning}");
            }
            require_hww_approval(yes, &proposal, "phone-key rotation")?;
            let approval = signer.approve_phone_rotation(&review.review_id, &request)?;
            match &request.rotation.renewed_policy {
                Some(_) => eprintln!(
                    "HWW validated and signed the phone-key rotation plus {} renewed-policy transactions",
                    approval.approval.policy_signatures.len()
                ),
                None => eprintln!("HWW validated and signed the phone-key rotation"),
            }
            write_artifact(&output, &approval)?;
            report_artifact(&output, "HWW-approved phone-key rotation")?;
        }
        HwwCommand::AdoptRotation { approved_rotation } => {
            let approval: ApprovedDevicePhoneRotation = read_artifact(&approved_rotation)?;
            let config = signer.adopt_phone_rotation(&approval.request_id)?;
            println!(
                "HWW paired with the rotated vault: {}",
                config.vault_address
            );
        }
    }
    Ok(())
}

fn print_active_policy(wallet: &WalletApplication) -> Result<()> {
    let config = wallet.configuration()?;
    println!("Cold storage descriptor: {}", config.vault_descriptor);
    println!("Vault address: {}", config.vault_address);
    println!(
        "Policy controller address: {}",
        controller_address(&config)?
    );
    println!(
        "Policy controller reserve: {} sats per active chain",
        anzen_protocol::consts::CONNECTOR_VALUE_SATS
    );
    print_recovery_delay(&config);
    if config.monthly_limit_sats == 0 {
        println!("Monthly spending: disabled");
    } else {
        println!("Monthly limit: {} sats", config.monthly_limit_sats);
        let schedule = anzen_common::storage::read_json::<hot_wallet::api::Schedule>(
            &wallet_data_dir(wallet).join(anzen_wallet::ceremony::SCHEDULE_FILE),
        );
        if let Ok(schedule) = schedule {
            println!(
                "Presigned allowance authorizations: {}",
                schedule.entries.len()
            );
            println!(
                "Allowance hop delay: {} seconds (~30 days)",
                schedule.monthly_delay_seconds
            );
        }
    }
    if config.emergency_access_limit_sats == 0 {
        println!("Emergency access: disabled");
    } else {
        println!(
            "Emergency access limit: {} sats",
            config.emergency_access_limit_sats
        );
        println!(
            "Emergency access delay: {} seconds (~1 week)",
            anzen_protocol::consts::EMERGENCY_ACCESS_DELAY_SECONDS
        );
    }
    Ok(())
}

/// Public chain status of the vault this directory knows: the phone's, the vault a replacement
/// phone is recovering, or the signer's when no phone remains.
fn print_status(
    data_dir: &Path,
    wallet: &WalletApplication,
    signer: &SignerApplication,
    rpc_args: &ChainArgs,
) -> Result<()> {
    // Status is a public, host-level view: it must work after both devices are lost.
    let config = anzen_common::storage::load_config(data_dir)
        .or_else(|_| wallet.recovery_source())
        .or_else(|_| signer.export_recovery_bundle().map(|bundle| bundle.config))?;
    let backend = rpc_args.connect_chain(config.bitcoin_network()?)?;
    let tip = backend.chain_tip()?;
    let utxos = backend.scan_vault(&config)?;
    let connectors = backend.scan_connectors(&config)?;
    let balance = utxos
        .iter()
        .map(|utxo| utxo.txout.value.to_sat())
        .sum::<u64>();
    println!("Network: {}", config.network);
    if tip.network != Network::Regtest {
        println!("Chain backend: {}", backend.backend_description());
    }
    println!("Height: {}", tip.height);
    println!("Median time past: {}", tip.median_time);
    println!("Vault UTXOs: {}", utxos.len());
    println!("Vault balance: {} sats", balance);
    println!("Live policy controller outputs: {}", connectors.len());
    println!(
        "Monthly spending: {}",
        if config.monthly_limit_sats == 0 {
            "disabled".to_owned()
        } else {
            format!("{} sats", config.monthly_limit_sats)
        }
    );
    println!(
        "Emergency access: {}",
        if config.emergency_access_limit_sats == 0 {
            "disabled".to_owned()
        } else {
            format!("{} sats", config.emergency_access_limit_sats)
        }
    );
    if let Some(oldest) = utxos.first() {
        println!("Oldest confirmation height: {}", oldest.confirmation_height);
        let clock = anzen_wallet::recovery::RecoveryClock::read(
            &*backend,
            &tip,
            std::slice::from_ref(oldest),
        )?;
        print_recovery_activation(clock.opens_at(oldest)?, tip.median_time);
    }
    Ok(())
}

fn run_node(command: NodeCommand, rpc_args: &ChainArgs, network: Network) -> Result<()> {
    if network != Network::Regtest {
        bail!("vault node commands are unavailable in mainnet mode");
    }
    let rpc = rpc_args.connect_core(Network::Regtest)?;
    match command {
        NodeCommand::Info => {
            let info = rpc.chain_info()?;
            println!("Network: {}", info.chain);
            println!("Height: {}", info.blocks);
            println!("Median time past: {}", info.median_time);
            println!("Best block: {}", info.best_block_hash);
        }
        NodeCommand::SetTime { timestamp } => {
            rpc.set_mock_time(timestamp)?;
            println!("Regtest mock time set to {timestamp}");
        }
        NodeCommand::Mine { blocks, address } => {
            let address = address
                .parse::<bitcoin::Address<_>>()?
                .require_network(Network::Regtest)?;
            let hashes = rpc.mine(blocks, &address)?;
            println!("Mined {} blocks", hashes.len());
            if let Some(hash) = hashes.last() {
                println!("New tip: {hash}");
            }
        }
    }
    Ok(())
}

fn controller_address(config: &VaultConfig) -> Result<String> {
    let mut crypto = SoftwareCrypto::new();
    let scripts = Scripts::new(&mut crypto, &anzen_common::protocol::vault_keys(config)?)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    address::from_script_pubkey(
        &mut crypto,
        &scripts.controller.script_pubkey,
        anzen_common::protocol::config_network(config)?,
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
}

fn output(stderr: bool) -> Box<dyn Write> {
    if stderr {
        Box::new(io::stderr().lock())
    } else {
        Box::new(io::stdout().lock())
    }
}

fn print_policy_request(request: &PolicyRequest, stderr: bool) -> Result<()> {
    let (scripts, graph) = anzen_wallet::ceremony::graph(request)?;
    let mut crypto = SoftwareCrypto::new();
    let address = |crypto: &mut SoftwareCrypto, script: &[u8]| {
        address::from_script_pubkey(crypto, script, request.network)
            .map_err(|error| anyhow::anyhow!("{error}"))
    };
    let limits = request.limits;
    let months = anzen_protocol::consts::MONTHS_PER_ROLLOVER;
    let mut output = output(stderr);
    writeln!(output, "PHONE POLICY PROPOSAL")?;
    writeln!(
        output,
        "Vault address: {}",
        address(&mut crypto, &scripts.vault.script_pubkey)?
    )?;
    writeln!(
        output,
        "Policy controller address: {}",
        address(&mut crypto, &scripts.controller.script_pubkey)?
    )?;
    writeln!(
        output,
        "Policy controller reserve: {} sats per active chain",
        anzen_protocol::consts::CONNECTOR_VALUE_SATS
    )?;
    if limits.monthly_limit_sats == 0 {
        writeln!(output, "Monthly spending: disabled")?;
    } else {
        writeln!(output, "Monthly limit: {} sats", limits.monthly_limit_sats)?;
    }
    if limits.emergency_access_limit_sats == 0 {
        writeln!(output, "Emergency access: disabled")?;
    } else {
        writeln!(
            output,
            "Emergency access limit: {} sats",
            limits.emergency_access_limit_sats
        )?;
        writeln!(
            output,
            "Emergency access delay: {} seconds (~1 week)",
            anzen_protocol::consts::EMERGENCY_ACCESS_DELAY_SECONDS
        )?;
    }
    writeln!(
        output,
        "Fee rate: {} sat/vB",
        anzen_protocol::consts::FEE_RATE_SAT_VB
    )?;
    writeln!(output, "Total input: {} sats", graph.plan.total_input_sats)?;
    writeln!(output, "Allowance steps: {}", graph.allowances.len())?;
    if limits.monthly_limit_sats > 0 {
        writeln!(
            output,
            "Allowance hop delay: {} seconds (~30 days)",
            anzen_protocol::consts::MONTHLY_ALLOWANCE_DELAY_SECONDS
        )?;
        if graph.allowances.len() < months {
            writeln!(
                output,
                "WARNING: balance funds only {} of {months} monthly allowances",
                graph.allowances.len()
            )?;
        }
    }
    writeln!(output, "Rollover txid: {}", graph.rollover.txid)?;
    writeln!(output, "Rollover fee: {} sats", graph.rollover.fee_sats)?;
    if !graph.allowances.is_empty() {
        writeln!(
            output,
            "Initial allowance-chain UTXO: {} sats",
            graph.plan.allowance_value_sats
        )?;
        writeln!(
            output,
            "Rollover remainder: {} sats",
            graph.plan.remainder_value_sats
        )?;
    }
    if let Some(emergency) = &graph.emergency {
        writeln!(output, "Emergency trigger txid: {}", emergency.trigger.txid)?;
        writeln!(
            output,
            "Emergency withdrawal txid: {}",
            emergency.withdrawal.txid
        )?;
        writeln!(
            output,
            "Emergency hot address: {}",
            address(&mut crypto, &emergency.hot_script)?
        )?;
    }
    writeln!(
        output,
        "Presigned transactions: {}",
        graph.transactions().len()
    )?;
    Ok(())
}

fn report_sweep(request: &SweepRequest, stderr: bool) -> Result<()> {
    let (_, spend) = request
        .build(&mut SoftwareCrypto::new())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut output = output(stderr);
    writeln!(output, "COOPERATIVE VAULT SWEEP")?;
    writeln!(output, "Destination: {}", request.destination)?;
    writeln!(output, "Inputs: {}", request.inputs.len())?;
    writeln!(output, "Sent: {} sats", spend.amount_sats)?;
    writeln!(output, "Fee: {} sats (1 sat/vB)", spend.fee_sats)?;
    Ok(())
}

fn report_rotation(request: &DevicePhoneRotation, stderr: bool) -> Result<()> {
    let plan = request
        .rotation
        .build(&mut SoftwareCrypto::new())
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let mut output = output(stderr);
    writeln!(output, "PHONE-KEY ROTATION")?;
    writeln!(
        output,
        "New phone vault key: {}",
        request.new_config.phone_vault_pubkey
    )?;
    writeln!(
        output,
        "New cold storage descriptor: {}",
        request.new_config.vault_descriptor
    )?;
    writeln!(
        output,
        "New vault address: {}",
        request.new_config.vault_address
    )?;
    writeln!(output, "Inputs: {}", plan.sweep.transaction.input.len())?;
    writeln!(output, "Sent: {} sats", plan.sweep.amount_sats)?;
    writeln!(output, "Fee: {} sats (1 sat/vB)", plan.sweep.fee_sats)?;
    if !request.rotation.revoke_connectors.is_empty() {
        writeln!(
            output,
            "Old policy controllers revoked: {}",
            request.rotation.revoke_connectors.len()
        )?;
    }
    match &plan.renewed {
        Some((_, graph)) => {
            writeln!(
                output,
                "Monthly policy preserved: {} sats",
                graph.limits.monthly_limit_sats
            )?;
            writeln!(
                output,
                "Renewed allowance steps: {}",
                graph.allowances.len()
            )?;
            writeln!(
                output,
                "Renewed policy transactions: {}",
                graph.transactions().len()
            )?;
            if graph.limits.emergency_access_limit_sats > 0 {
                writeln!(
                    output,
                    "Emergency access preserved: {} sats",
                    graph.limits.emergency_access_limit_sats
                )?;
            }
        }
        None => writeln!(output, "Programmable vault policy remains disabled")?,
    }
    Ok(())
}

fn require_hww_approval(yes: bool, input: &Path, description: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    if input == Path::new("-") {
        bail!("--yes is required when the proposal is read from stdin");
    }
    print!("Type `approve` to confirm the {description}: ");
    io::stdout().flush()?;
    let mut response = String::new();
    io::stdin().read_line(&mut response)?;
    if response.trim() != "approve" {
        bail!("HWW approval declined");
    }
    Ok(())
}

fn print_sweep_result(label: &str, result: &hot_wallet::api::SweepResult) {
    println!("{label}: {}", result.txid);
    println!("Inputs: {}", result.input_count);
    println!("Sent: {} sats", result.sent_sats);
    println!("Fee: {} sats (1 sat/vB)", result.fee_sats);
}

fn wallet_data_dir(wallet: &WalletApplication) -> PathBuf {
    wallet.data_dir().to_path_buf()
}

fn read_artifact<T: DeserializeOwned>(path: &Path) -> Result<T> {
    if path == Path::new("-") {
        serde_json::from_reader(io::stdin().lock())
            .context("failed to read JSON artifact from stdin")
    } else {
        anzen_common::storage::read_json(path)
    }
}

fn write_artifact<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if path == Path::new("-") {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, value)?;
        writeln!(stdout)?;
        Ok(())
    } else {
        anzen_common::storage::write_json(path, value)
    }
}

fn artifact_reports_to_stderr(path: &Path) -> bool {
    path == Path::new("-")
}

fn report_artifact(path: &Path, description: &str) -> Result<()> {
    if path != Path::new("-") {
        println!("{description}: {}", path.display());
    }
    Ok(())
}

fn command_network(
    data_dir: &Path,
    dangerously_enable_mainnet: bool,
    requested: Option<TestNetwork>,
) -> Result<Network> {
    let configured = if anzen_common::storage::config_exists(data_dir) {
        Some(anzen_common::storage::load_config(data_dir)?.bitcoin_network()?)
    } else if data_dir
        .join(anzen_common::storage::PHONE_DEVICE_FILE)
        .exists()
    {
        Some(
            anzen_common::storage::load_device(data_dir, anzen_common::storage::PHONE_DEVICE_FILE)?
                .bitcoin_network()?,
        )
    } else if data_dir
        .join(anzen_common::storage::HWW_DEVICE_FILE)
        .exists()
    {
        Some(
            anzen_common::storage::load_device(data_dir, anzen_common::storage::HWW_DEVICE_FILE)?
                .bitcoin_network()?,
        )
    } else {
        None
    };

    let requested = requested.map(TestNetwork::bitcoin);
    if dangerously_enable_mainnet && requested.is_some() {
        bail!("--network selects a test network; it cannot be combined with mainnet");
    }
    match (configured, dangerously_enable_mainnet) {
        (Some(Network::Bitcoin), false) => {
            bail!("mainnet vault is locked; pass --dangerously-enable-mainnet on every command")
        }
        (Some(Network::Bitcoin), true) => Ok(Network::Bitcoin),
        (Some(network), true) => bail!(
            "vault is configured for {}; --dangerously-enable-mainnet cannot change an existing vault",
            anzen_common::storage::network_name(network)
        ),
        (Some(network), false) if requested.is_some_and(|requested| requested != network) => {
            bail!(
                "vault is configured for {}; --network cannot change an existing vault",
                anzen_common::storage::network_name(network)
            )
        }
        (Some(network @ (Network::Regtest | Network::Signet)), false) => Ok(network),
        (Some(other), _) => bail!("unsupported vault network: {other}"),
        (None, true) => Ok(Network::Bitcoin),
        (None, false) => Ok(requested.unwrap_or(Network::Regtest)),
    }
}

fn network_label(network: Network) -> &'static str {
    match network {
        Network::Bitcoin => "MAINNET — REAL FUNDS",
        Network::Signet => "MUTINYNET — TEST COINS",
        Network::Regtest => "REGTEST ONLY",
        _ => "UNSUPPORTED NETWORK",
    }
}

fn print_recovery_delay(config: &VaultConfig) {
    println!(
        "HWW alone:      after {} seconds (~388 days); renewing the vault restarts it",
        format_number(u64::from(config.hww_recovery_seconds))
    );
}

/// The signer's solo path opens once the chain's median time past reaches `opens_at`.
fn print_recovery_activation(opens_at: u64, median_time: u64) {
    let date = i64::try_from(opens_at)
        .ok()
        .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
        .map(|date| date.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "outside displayable range".to_owned());
    if median_time >= opens_at {
        println!("HWW recovery: ACTIVE (opened at median time {date})");
    } else {
        println!(
            "HWW recovery: opens at median time {date} ({} days remaining)",
            (opens_at - median_time).div_ceil(86_400)
        );
    }
}

fn vanity_engine(cpu: bool) -> hot_wallet::VanityEngine {
    if cpu {
        hot_wallet::VanityEngine::Cpu
    } else {
        hot_wallet::VanityEngine::Gpu
    }
}

fn engine_label(engine: hot_wallet::VanityEngine) -> &'static str {
    match engine {
        hot_wallet::VanityEngine::Gpu => "on the GPU",
        hot_wallet::VanityEngine::Cpu => "across all available CPU threads",
    }
}

fn report_vanity_progress(attempts: u64) {
    eprintln!(
        "Vanity search: {} candidates tested...",
        format_number(attempts)
    );
}

/// Point a failed GPU search at the CPU fallback.
fn gpu_hint(error: anyhow::Error, engine: hot_wallet::VanityEngine) -> anyhow::Error {
    match engine {
        hot_wallet::VanityEngine::Gpu => {
            error.context("GPU vanity search failed; pass --cpu to grind on the CPU")
        }
        hot_wallet::VanityEngine::Cpu => error,
    }
}

fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_chain_args() -> ChainArgs {
        ChainArgs {
            chain_backend: None,
            rpc_url: None,
            rpc_user: "anzen".to_owned(),
            rpc_password: "anzen".to_owned(),
            electrum_url: None,
        }
    }

    #[test]
    fn chain_backend_defaults_can_be_overridden_on_either_network() {
        let mut args = default_chain_args();
        assert_eq!(
            args.selected_backend(Network::Regtest).unwrap(),
            ChainBackendKind::Rpc
        );
        assert_eq!(
            args.selected_backend(Network::Bitcoin).unwrap(),
            ChainBackendKind::Electrum
        );
        assert_eq!(
            args.selected_backend(Network::Signet).unwrap(),
            ChainBackendKind::Electrum
        );
        assert!(args.connect_core(Network::Signet).is_err());

        args.chain_backend = Some(ChainBackendKind::Electrum);
        assert_eq!(
            args.selected_backend(Network::Regtest).unwrap(),
            ChainBackendKind::Electrum
        );
        args.chain_backend = Some(ChainBackendKind::Rpc);
        assert_eq!(
            args.selected_backend(Network::Bitcoin).unwrap(),
            ChainBackendKind::Rpc
        );
    }
}
