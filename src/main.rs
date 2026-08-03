use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "vault-cli", version, about)]
struct Cli {
    /// Directory containing simulated device, cloud, and wallet state.
    #[arg(long, default_value = ".vault-data", global = true)]
    data_dir: PathBuf,

    #[command(flatten)]
    rpc: RpcArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create simulated phone/HWW keys and the static vault policy.
    Init {
        /// Monthly hard limit in satoshis.
        #[arg(long, default_value_t = vault_cli::DEFAULT_HARD_LIMIT_SATS)]
        hard_limit_sats: u64,
    },
    /// Print the configured high-level vault policy.
    Policy,
    /// Print vault and chain balances from the real regtest node.
    Status,
    /// Derive and persist the next mobile hot-wallet receive address.
    HotAddress,
    /// Regtest-only node controls used by the end-to-end demonstration.
    Node {
        #[command(subcommand)]
        command: NodeCommand,
    },
    /// Prepare, approve, and finalize the annual batch signing ceremony.
    Ceremony {
        #[command(subcommand)]
        command: CeremonyCommand,
    },
    /// Broadcast one encrypted monthly authorization or revocation.
    Monthly {
        month: String,
        #[arg(value_enum)]
        action: MonthlyAction,
    },
    /// Return the difference between a claimed hard allowance and a lower soft limit to the vault.
    SoftLimit { month: String, soft_limit_sats: u64 },
    /// Restore a deleted phone key from its HWW-encrypted cloud backup.
    RestorePhone,
    /// Sweep all currently mature vault UTXOs through one policy path.
    Sweep {
        #[arg(value_enum)]
        path: CliSweepPath,
        destination: String,
    },
    /// Cooperatively move all funds to a newly generated phone-key epoch.
    RotatePhone,
}

#[derive(Debug, Clone, Args)]
struct RpcArgs {
    #[arg(
        long,
        env = "VAULT_RPC_URL",
        default_value = "http://127.0.0.1:18443",
        global = true
    )]
    rpc_url: String,
    #[arg(long, env = "VAULT_RPC_USER", default_value = "vault", global = true)]
    rpc_user: String,
    #[arg(
        long,
        env = "VAULT_RPC_PASSWORD",
        default_value = "vault",
        global = true
    )]
    rpc_password: String,
}

impl RpcArgs {
    fn connect(&self) -> Result<vault_cli::rpc::RegtestRpc> {
        vault_cli::rpc::RegtestRpc::connect(&vault_cli::rpc::RpcConfig {
            url: self.rpc_url.clone(),
            user: self.rpc_user.clone(),
            password: self.rpc_password.clone(),
        })
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

#[derive(Debug, Subcommand)]
enum CeremonyCommand {
    /// Build the rollover and all monthly PSBTs, then add the phone signatures.
    Prepare {
        /// Override the captured current Unix time (test-only).
        #[arg(long)]
        now: Option<i64>,
        /// Override the ceremony directory.
        #[arg(long)]
        batch_dir: Option<PathBuf>,
    },
    /// Show the high-level policy and transaction batch awaiting approval.
    Show {
        #[arg(long)]
        batch_dir: Option<PathBuf>,
    },
    /// Validate the complete policy once and add all simulated HWW signatures.
    Approve {
        #[arg(long)]
        batch_dir: Option<PathBuf>,
        /// Approve non-interactively; intended for the deterministic demo.
        #[arg(long)]
        yes: bool,
    },
    /// Finalize every PSBT, encrypt monthly transactions, and broadcast rollover.
    Finalize {
        #[arg(long)]
        batch_dir: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MonthlyAction {
    Authorize,
    Revoke,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliSweepPath {
    Cooperative,
    PhoneRecovery,
    HwwRecovery,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { hard_limit_sats } => {
            let initialized = vault_cli::state::initialize(&cli.data_dir, hard_limit_sats)?;
            println!("Vault initialized (REGTEST ONLY)");
            println!("Phone mnemonic: {}", initialized.phone_mnemonic);
            println!("HWW mnemonic:   {}", initialized.hww_mnemonic);
            println!("Phone vault key: {}", initialized.config.phone_vault_pubkey);
            println!("HWW vault key:   {}", initialized.config.hww_vault_pubkey);
            println!("Descriptor: {}", initialized.config.vault_descriptor);
            println!("Vault address: {}", initialized.config.vault_address);
            println!(
                "Phone recovery: {} blocks",
                initialized.config.phone_recovery_blocks
            );
            println!(
                "HWW recovery:   {} blocks",
                initialized.config.hww_recovery_blocks
            );
            println!(
                "Hard limit:     {} sats",
                initialized.config.hard_limit_sats
            );
        }
        Command::Policy => {
            let config = vault_cli::state::load_config(&cli.data_dir)?;
            println!("Descriptor: {}", config.vault_descriptor);
            println!("Vault address: {}", config.vault_address);
            println!("Phone recovery: {} blocks", config.phone_recovery_blocks);
            println!("HWW recovery:   {} blocks", config.hww_recovery_blocks);
            println!("Hard limit:     {} sats", config.hard_limit_sats);
        }
        Command::Status => {
            let config = vault_cli::state::load_config(&cli.data_dir)?;
            let rpc = cli.rpc.connect()?;
            let info = rpc.chain_info()?;
            let utxos = rpc.scan_vault(&config)?;
            let balance = utxos
                .iter()
                .map(|utxo| utxo.txout.value.to_sat())
                .sum::<u64>();
            println!("Network: regtest");
            println!("Height: {}", info.blocks);
            println!("Median time past: {}", info.median_time);
            println!("Vault UTXOs: {}", utxos.len());
            println!("Vault balance: {} sats", balance);
            if let Some(oldest) = utxos.first() {
                println!("Oldest confirmation height: {}", oldest.confirmation_height);
                print_recovery_activation(
                    "Phone",
                    oldest.confirmation_height + u64::from(config.phone_recovery_blocks),
                    info.blocks + 1,
                );
                print_recovery_activation(
                    "HWW",
                    oldest.confirmation_height + u64::from(config.hww_recovery_blocks),
                    info.blocks + 1,
                );
            }
        }
        Command::HotAddress => {
            let mut hot = vault_cli::hot::HotWallet::open_or_create(&cli.data_dir)?;
            let rpc = cli.rpc.connect()?;
            hot.sync(&rpc.client)?;
            println!("Hot receive address: {}", hot.next_receive_address()?);
        }
        Command::Node { command } => {
            let rpc = cli.rpc.connect()?;
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
                    use bitcoin::Address;
                    use std::str::FromStr;
                    let address =
                        Address::from_str(&address)?.require_network(bitcoin::Network::Regtest)?;
                    let hashes = rpc.mine(blocks, &address)?;
                    println!("Mined {} blocks", hashes.len());
                    if let Some(hash) = hashes.last() {
                        println!("New tip: {hash}");
                    }
                }
            }
        }
        Command::Ceremony { command } => match command {
            CeremonyCommand::Prepare { now, batch_dir } => {
                let rpc = cli.rpc.connect()?;
                let timestamp = now.unwrap_or_else(|| chrono::Utc::now().timestamp());
                let now = chrono::DateTime::from_timestamp(timestamp, 0)
                    .ok_or_else(|| anyhow::anyhow!("invalid ceremony timestamp {timestamp}"))?;
                let batch_dir = batch_dir
                    .unwrap_or_else(|| vault_cli::ceremony::default_batch_path(&cli.data_dir));
                let manifest = vault_cli::ceremony::prepare(&cli.data_dir, &rpc, now, &batch_dir)?;
                print_manifest(&manifest, &batch_dir);
                println!(
                    "Phone approved and signed all {} PSBTs",
                    1 + manifest.chunk_count * 2
                );
                println!("Next: review with `vault-cli ceremony approve`");
            }
            CeremonyCommand::Show { batch_dir } => {
                let batch_dir = batch_dir
                    .unwrap_or_else(|| vault_cli::ceremony::default_batch_path(&cli.data_dir));
                let manifest = vault_cli::ceremony::load_manifest(&batch_dir)?;
                print_manifest(&manifest, &batch_dir);
            }
            CeremonyCommand::Approve { batch_dir, yes } => {
                let batch_dir = batch_dir
                    .unwrap_or_else(|| vault_cli::ceremony::default_batch_path(&cli.data_dir));
                let manifest = vault_cli::ceremony::load_manifest(&batch_dir)?;
                println!("SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL");
                print_manifest(&manifest, &batch_dir);
                if !yes {
                    use std::io::{self, Write};
                    print!("Type `approve` to sign the complete batch: ");
                    io::stdout().flush()?;
                    let mut response = String::new();
                    io::stdin().read_line(&mut response)?;
                    if response.trim() != "approve" {
                        anyhow::bail!("HWW policy approval declined");
                    }
                }
                let approved = vault_cli::ceremony::approve_hww(&cli.data_dir, &batch_dir)?;
                println!(
                    "HWW validated and signed all {} PSBTs after one approval",
                    1 + approved.chunk_count * 2
                );
            }
            CeremonyCommand::Finalize { batch_dir } => {
                let rpc = cli.rpc.connect()?;
                let batch_dir = batch_dir
                    .unwrap_or_else(|| vault_cli::ceremony::default_batch_path(&cli.data_dir));
                let schedule =
                    vault_cli::ceremony::finalize_and_broadcast(&cli.data_dir, &rpc, &batch_dir)?;
                println!("Rollover broadcast: {}", schedule.rollover_txid);
                println!(
                    "Encrypted monthly transaction pairs: {}",
                    schedule.entries.len()
                );
                for entry in schedule.entries {
                    println!(
                        "{} unlock={} authorization={} revocation={}",
                        entry.month,
                        entry.unlock_timestamp,
                        entry.authorization_txid,
                        entry.revocation_txid
                    );
                }
            }
        },
        Command::Monthly { month, action } => {
            let rpc = cli.rpc.connect()?;
            let kind = match action {
                MonthlyAction::Authorize => vault_cli::ceremony::TransactionKind::Authorization,
                MonthlyAction::Revoke => vault_cli::ceremony::TransactionKind::Revocation,
            };
            let txid = vault_cli::ceremony::broadcast_monthly(&cli.data_dir, &rpc, &month, kind)?;
            println!("Broadcast {action:?} for {month}: {txid}");
        }
        Command::SoftLimit {
            month,
            soft_limit_sats,
        } => {
            let rpc = cli.rpc.connect()?;
            match vault_cli::ceremony::apply_soft_limit(
                &cli.data_dir,
                &rpc,
                &month,
                soft_limit_sats,
            )? {
                Some(txid) => println!(
                    "Soft limit applied for {month}: retained at most {soft_limit_sats} sats hot; cold-return txid={txid}"
                ),
                None => println!(
                    "Soft limit equals hard limit for {month}; no cold-return transaction required"
                ),
            }
        }
        Command::RestorePhone => {
            let mnemonic = vault_cli::recovery::restore_phone_from_hww_backup(&cli.data_dir)?;
            println!("Phone key restored from HWW-encrypted backup");
            println!("Recovered phone mnemonic: {mnemonic}");
        }
        Command::Sweep { path, destination } => {
            use bitcoin::Address;
            use std::str::FromStr;
            let destination =
                Address::from_str(&destination)?.require_network(bitcoin::Network::Regtest)?;
            let path = match path {
                CliSweepPath::Cooperative => vault_cli::recovery::SweepPath::Cooperative,
                CliSweepPath::PhoneRecovery => vault_cli::recovery::SweepPath::PhoneRecovery,
                CliSweepPath::HwwRecovery => vault_cli::recovery::SweepPath::HwwRecovery,
            };
            let rpc = cli.rpc.connect()?;
            let result = vault_cli::recovery::sweep(&cli.data_dir, &rpc, path, &destination)?;
            println!("Vault sweep broadcast via {path:?}: {}", result.txid);
            println!("Inputs: {}", result.input_count);
            println!("Sent: {} sats", result.sent_sats);
            println!("Fee: {} sats (1 sat/vB)", result.fee_sats);
        }
        Command::RotatePhone => {
            let rpc = cli.rpc.connect()?;
            let result = vault_cli::recovery::rotate_phone(&cli.data_dir, &rpc)?;
            println!(
                "Emergency phone-key rotation broadcast: {}",
                result.sweep.txid
            );
            println!("Old vault address: {}", result.old_address);
            println!("New vault address: {}", result.new_address);
            println!("New phone mnemonic: {}", result.new_phone_mnemonic);
            println!(
                "Moved {} sats from {} inputs; fee={} sats",
                result.sweep.sent_sats, result.sweep.input_count, result.sweep.fee_sats
            );
            println!("Old monthly authorizations are invalidated by the sweep");
        }
    }
    Ok(())
}

fn print_recovery_activation(label: &str, valid_height: u64, next_height: u64) {
    if next_height >= valid_height {
        println!("{label} recovery: ACTIVE (valid next-block height {valid_height})");
    } else {
        println!(
            "{label} recovery: valid next-block height {valid_height} ({} blocks remaining)",
            valid_height - next_height
        );
    }
}

fn print_manifest(manifest: &vault_cli::ceremony::BatchManifest, batch_dir: &std::path::Path) {
    println!("Batch directory: {}", batch_dir.display());
    println!("Vault address: {}", manifest.vault_address);
    println!("Descriptor: {}", manifest.vault_descriptor);
    println!("Hard limit: {} sats", manifest.hard_limit_sats);
    println!("Fee rate: {} sat/vB", manifest.fee_rate_sat_vb);
    println!("Total input: {} sats", manifest.total_input_sats);
    println!("Equal chunks: {}", manifest.chunk_count);
    println!("Rollover txid: {}", manifest.rollover.unsigned_txid);
    println!("Rollover fee: {} sats", manifest.rollover.fee_sats);
    for month in &manifest.months {
        println!(
            "{} unlock={} chunk={} sats hot={} auth={} revoke={}",
            month.month,
            month.unlock_timestamp,
            month.chunk_value_sats,
            month.hot_address,
            month.authorization.unsigned_txid,
            month.revocation.unsigned_txid
        );
    }
    println!("Phone approved: {}", manifest.phone_approved);
    println!("HWW approved: {}", manifest.hww_approved);
}
