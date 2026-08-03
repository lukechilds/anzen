use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(name = "vault", version, about)]
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
    /// Create the static cold-storage policy from initialized device keys.
    Init,
    /// Print the configured cold-storage and active monthly policy.
    Policy,
    /// Print vault and chain balances from the real regtest node.
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
    Init,
    /// Derive and persist the next hot-wallet receive address.
    ReceiveAddress,
    /// Send an exact amount from the phone hot wallet.
    Send { address: String, amount_sats: u64 },
    /// Propose a monthly policy and add all phone signatures.
    SetPolicy {
        #[arg(long)]
        monthly_limit: u64,
        #[arg(long)]
        output: PathBuf,
        /// Override the captured current Unix time (regtest-only).
        #[arg(long, hide = true)]
        now: Option<i64>,
    },
    /// Verify an HWW-approved policy, broadcast its rollover, and store artifacts.
    ActivatePolicy { approved_policy: PathBuf },
    /// Broadcast a presigned monthly authorization.
    Authorize { month: String },
    /// Return the difference between an authorization and a lower soft limit.
    ApplySoftLimit {
        month: String,
        #[arg(long)]
        limit: u64,
    },
    /// Broadcast a presigned monthly revocation.
    Revoke { month: String },
    /// Restore the phone key from an HWW-created recovery package.
    Restore { recovery: PathBuf },
    /// Sweep mature vault outputs using the phone-only recovery path.
    Recover { destination: String },
    /// Create and phone-sign a cooperative sweep proposal.
    CreateSweep {
        destination: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify and broadcast an HWW-approved cooperative sweep.
    BroadcastSweep { approved_sweep: PathBuf },
    /// Propose a new phone key while preserving any active monthly policy.
    RotateKey {
        #[arg(long)]
        output: PathBuf,
    },
    /// Activate an HWW-approved key rotation and replacement monthly schedule.
    ActivateRotation { approved_rotation: PathBuf },
}

#[derive(Debug, Subcommand)]
enum HwwCommand {
    /// Create the simulated HWW key and encrypt the phone backup.
    Init,
    /// Validate one high-level policy and sign its complete PSBT batch.
    ConfirmPolicy {
        proposal: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Approve non-interactively; intended for deterministic tests.
        #[arg(long)]
        yes: bool,
    },
    /// Decrypt a cloud phone backup into a portable recovery package.
    DecryptPhoneBackup {
        backup: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Sweep mature vault outputs using the HWW-only recovery path.
    Recover { destination: String },
    /// Validate and sign a phone-created cooperative sweep.
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init => initialize_vault(&cli.data_dir),
        Command::Policy => print_active_policy(&cli.data_dir),
        Command::Status => print_status(&cli.data_dir, &cli.rpc),
        Command::Phone { command } => run_phone(command, &cli.data_dir, &cli.rpc),
        Command::Hww { command } => run_hww(command, &cli.data_dir, &cli.rpc),
        Command::Node { command } => run_node(command, &cli.rpc),
    }
}

fn initialize_vault(data_dir: &Path) -> Result<()> {
    let config = vault_cli::state::initialize_vault(data_dir)?;
    println!("Vault initialized (REGTEST ONLY)");
    println!("Cold storage descriptor: {}", config.vault_descriptor);
    println!("Vault address: {}", config.vault_address);
    println!(
        "Phone recovery: {} blocks (~14 months)",
        format_number(u64::from(config.phone_recovery_blocks))
    );
    println!(
        "HWW recovery:   {} blocks (~15 months)",
        format_number(u64::from(config.hww_recovery_blocks))
    );
    println!("Monthly spending: disabled");
    Ok(())
}

fn run_phone(command: PhoneCommand, data_dir: &Path, rpc_args: &RpcArgs) -> Result<()> {
    match command {
        PhoneCommand::Init => {
            let phone = vault_cli::state::initialize_phone(data_dir)?;
            println!("Simulated phone initialized (REGTEST ONLY)");
            println!("Phone mnemonic: {}", phone.mnemonic);
            println!("Phone vault key: {}", phone.vault_pubkey);
        }
        PhoneCommand::ReceiveAddress => {
            let mut hot = vault_cli::hot::HotWallet::open_or_create(data_dir)?;
            let rpc = rpc_args.connect()?;
            hot.sync(&rpc.client)?;
            println!("Hot receive address: {}", hot.next_receive_address()?);
        }
        PhoneCommand::Send {
            address,
            amount_sats,
        } => phone_send(data_dir, rpc_args, &address, amount_sats)?,
        PhoneCommand::SetPolicy {
            monthly_limit,
            output,
            now,
        } => phone_set_policy(data_dir, rpc_args, monthly_limit, now, &output)?,
        PhoneCommand::ActivatePolicy { approved_policy } => {
            phone_activate_policy(data_dir, rpc_args, &approved_policy)?
        }
        PhoneCommand::Authorize { month } => {
            broadcast_monthly(
                data_dir,
                rpc_args,
                &month,
                vault_cli::ceremony::TransactionKind::Authorization,
            )?;
        }
        PhoneCommand::ApplySoftLimit { month, limit } => {
            let rpc = rpc_args.connect()?;
            match vault_cli::ceremony::apply_soft_limit(data_dir, &rpc, &month, limit)? {
                Some(txid) => println!(
                    "Soft limit applied for {month}: retained at most {limit} sats hot; cold-return txid={txid}"
                ),
                None => println!(
                    "Soft limit equals the monthly limit for {month}; no cold-return transaction required"
                ),
            }
        }
        PhoneCommand::Revoke { month } => {
            broadcast_monthly(
                data_dir,
                rpc_args,
                &month,
                vault_cli::ceremony::TransactionKind::Revocation,
            )?;
        }
        PhoneCommand::Restore { recovery } => {
            let package: vault_cli::recovery::PhoneRecoveryPackage = read_artifact(&recovery)?;
            let mnemonic = vault_cli::recovery::restore_phone_package(data_dir, &package)?;
            println!("Phone key restored from HWW recovery package");
            println!("Recovered phone mnemonic: {mnemonic}");
        }
        PhoneCommand::Recover { destination } => {
            device_recover(
                data_dir,
                rpc_args,
                &destination,
                vault_cli::recovery::SweepPath::PhoneRecovery,
            )?;
        }
        PhoneCommand::CreateSweep {
            destination,
            output,
        } => phone_create_sweep(data_dir, rpc_args, &destination, &output)?,
        PhoneCommand::BroadcastSweep { approved_sweep } => {
            let package: vault_cli::recovery::CooperativeSweepPackage =
                read_artifact(&approved_sweep)?;
            let rpc = rpc_args.connect()?;
            let result =
                vault_cli::recovery::broadcast_cooperative_sweep(data_dir, &rpc, &package)?;
            print_sweep_result("Cooperative vault sweep broadcast", &result);
        }
        PhoneCommand::RotateKey { output } => {
            let rpc = rpc_args.connect()?;
            let package = vault_cli::recovery::create_phone_rotation(data_dir, &rpc)?;
            report_rotation(&package, artifact_reports_to_stderr(&output))?;
            write_artifact(&output, &package)?;
            report_artifact(&output, "Phone-key rotation proposal")?;
        }
        PhoneCommand::ActivateRotation { approved_rotation } => {
            let package: vault_cli::recovery::PhoneRotationPackage =
                read_artifact(&approved_rotation)?;
            let rpc = rpc_args.connect()?;
            let result = vault_cli::recovery::activate_phone_rotation(data_dir, &rpc, &package)?;
            println!(
                "Emergency phone-key rotation broadcast: {}",
                result.sweep.txid
            );
            println!("Old vault address: {}", result.old_address);
            println!("New vault address: {}", result.new_address);
            println!("New phone mnemonic: {}", result.new_phone_mnemonic);
            match result.renewed_schedule {
                Some(schedule) => {
                    println!(
                        "Monthly policy preserved: {} sats",
                        schedule.monthly_limit_sats
                    );
                    println!("Policy rollover broadcast: {}", schedule.rollover_txid);
                    println!(
                        "Encrypted monthly transaction pairs: {}",
                        schedule.entries.len()
                    );
                }
                None => println!("Monthly spending remains disabled"),
            }
        }
    }
    Ok(())
}

fn run_hww(command: HwwCommand, data_dir: &Path, rpc_args: &RpcArgs) -> Result<()> {
    match command {
        HwwCommand::Init => {
            let hww = vault_cli::state::initialize_hww(data_dir)?;
            println!("Simulated HWW initialized (REGTEST ONLY)");
            println!("HWW mnemonic: {}", hww.mnemonic);
            println!("HWW vault key: {}", hww.vault_pubkey);
            println!("Phone backup encrypted for the HWW");
        }
        HwwCommand::ConfirmPolicy {
            proposal,
            output,
            yes,
        } => hww_confirm_policy(data_dir, &proposal, &output, yes)?,
        HwwCommand::DecryptPhoneBackup { backup, output } => {
            let package = vault_cli::recovery::decrypt_phone_backup_package(data_dir, &backup)?;
            write_artifact(&output, &package)?;
            report_artifact(&output, "Decrypted phone recovery package")?;
        }
        HwwCommand::Recover { destination } => {
            device_recover(
                data_dir,
                rpc_args,
                &destination,
                vault_cli::recovery::SweepPath::HwwRecovery,
            )?;
        }
        HwwCommand::ConfirmSweep {
            proposal,
            output,
            yes,
        } => {
            let package: vault_cli::recovery::CooperativeSweepPackage = read_artifact(&proposal)?;
            print_hww_sweep_prompt(&package, yes, proposal.as_path())?;
            let approved = vault_cli::recovery::confirm_cooperative_sweep(data_dir, &package)?;
            eprintln!("HWW validated and signed the cooperative sweep");
            write_artifact(&output, &approved)?;
            report_artifact(&output, "HWW-approved cooperative sweep")?;
        }
        HwwCommand::ConfirmRotation {
            proposal,
            output,
            yes,
        } => {
            let package: vault_cli::recovery::PhoneRotationPackage = read_artifact(&proposal)?;
            report_rotation(&package, true)?;
            require_hww_approval(yes, proposal.as_path(), "phone-key rotation")?;
            let approved = vault_cli::recovery::confirm_phone_rotation(data_dir, &package)?;
            if let Some(policy) = &approved.renewed_policy {
                eprintln!(
                    "HWW validated and signed the phone-key rotation plus {} renewed-policy PSBTs",
                    1 + policy.manifest.chunk_count * 2
                );
            } else {
                eprintln!("HWW validated and signed the phone-key rotation");
            }
            write_artifact(&output, &approved)?;
            report_artifact(&output, "HWW-approved phone-key rotation")?;
        }
    }
    Ok(())
}

fn phone_set_policy(
    data_dir: &Path,
    rpc_args: &RpcArgs,
    monthly_limit: u64,
    now: Option<i64>,
    output: &Path,
) -> Result<()> {
    let rpc = rpc_args.connect()?;
    let timestamp = now.unwrap_or_else(|| chrono::Utc::now().timestamp());
    let now = chrono::DateTime::from_timestamp(timestamp, 0)
        .with_context(|| format!("invalid policy timestamp {timestamp}"))?;
    let workspace = data_dir.join("phone/policy-proposal");
    reset_workspace(&workspace)?;
    let manifest = vault_cli::ceremony::prepare(data_dir, &rpc, now, monthly_limit, &workspace)?;
    let package = vault_cli::ceremony::package_from_batch(&workspace)?;
    print_manifest(&manifest, artifact_reports_to_stderr(output))?;
    write_artifact(output, &package)?;
    report_artifact(output, "Phone-signed policy proposal")?;
    Ok(())
}

fn hww_confirm_policy(data_dir: &Path, proposal: &Path, output: &Path, yes: bool) -> Result<()> {
    let package: vault_cli::ceremony::PolicyPackage = read_artifact(proposal)?;
    eprintln!("SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL");
    print_manifest(&package.manifest, true)?;
    require_hww_approval(yes, proposal, "complete monthly policy")?;
    let workspace = data_dir.join("hww/policy-review");
    reset_workspace(&workspace)?;
    vault_cli::ceremony::materialize_policy_package(&package, &workspace)?;
    let approved_manifest = vault_cli::ceremony::approve_hww(data_dir, &workspace)?;
    eprintln!(
        "HWW validated and signed all {} PSBTs after one approval",
        1 + approved_manifest.chunk_count * 2
    );
    let approved = vault_cli::ceremony::package_from_batch(&workspace)?;
    write_artifact(output, &approved)?;
    report_artifact(output, "HWW-approved policy")?;
    Ok(())
}

fn phone_activate_policy(data_dir: &Path, rpc_args: &RpcArgs, approved: &Path) -> Result<()> {
    let package: vault_cli::ceremony::PolicyPackage = read_artifact(approved)?;
    if !package.manifest.phone_approved || !package.manifest.hww_approved {
        bail!("policy package requires both phone and HWW approval");
    }
    let workspace = data_dir.join("phone/policy-activation");
    reset_workspace(&workspace)?;
    vault_cli::ceremony::materialize_policy_package(&package, &workspace)?;
    let rpc = rpc_args.connect()?;
    let schedule = vault_cli::ceremony::finalize_and_broadcast(data_dir, &rpc, &workspace)?;
    vault_cli::state::set_monthly_limit(data_dir, package.manifest.monthly_limit_sats)?;
    println!("Rollover broadcast: {}", schedule.rollover_txid);
    println!("Active monthly limit: {} sats", schedule.monthly_limit_sats);
    println!(
        "Encrypted monthly transaction pairs: {}",
        schedule.entries.len()
    );
    Ok(())
}

fn phone_create_sweep(
    data_dir: &Path,
    rpc_args: &RpcArgs,
    destination: &str,
    output: &Path,
) -> Result<()> {
    let address = regtest_address(destination)?;
    let rpc = rpc_args.connect()?;
    let package = vault_cli::recovery::create_cooperative_sweep(data_dir, &rpc, &address)?;
    report_sweep(&package, artifact_reports_to_stderr(output))?;
    write_artifact(output, &package)?;
    report_artifact(output, "Phone-signed cooperative sweep")?;
    Ok(())
}

fn print_hww_sweep_prompt(
    package: &vault_cli::recovery::CooperativeSweepPackage,
    yes: bool,
    proposal: &Path,
) -> Result<()> {
    report_sweep(package, true)?;
    require_hww_approval(yes, proposal, "cooperative sweep")
}

fn device_recover(
    data_dir: &Path,
    rpc_args: &RpcArgs,
    destination: &str,
    path: vault_cli::recovery::SweepPath,
) -> Result<()> {
    let destination = regtest_address(destination)?;
    let rpc = rpc_args.connect()?;
    let result = vault_cli::recovery::sweep(data_dir, &rpc, path, &destination)?;
    let label = match path {
        vault_cli::recovery::SweepPath::PhoneRecovery => "Phone recovery sweep broadcast",
        vault_cli::recovery::SweepPath::HwwRecovery => "HWW recovery sweep broadcast",
        vault_cli::recovery::SweepPath::Cooperative => "Cooperative sweep broadcast",
    };
    print_sweep_result(label, &result);
    Ok(())
}

fn broadcast_monthly(
    data_dir: &Path,
    rpc_args: &RpcArgs,
    month: &str,
    kind: vault_cli::ceremony::TransactionKind,
) -> Result<()> {
    let rpc = rpc_args.connect()?;
    let txid = vault_cli::ceremony::broadcast_monthly(data_dir, &rpc, month, kind)?;
    let action = match kind {
        vault_cli::ceremony::TransactionKind::Authorization => "Authorization",
        vault_cli::ceremony::TransactionKind::Revocation => "Revocation",
    };
    println!("Broadcast {action} for {month}: {txid}");
    Ok(())
}

fn phone_send(data_dir: &Path, rpc_args: &RpcArgs, address: &str, amount_sats: u64) -> Result<()> {
    use bitcoincore_rpc::RpcApi;
    let address = regtest_address(address)?;
    let rpc = rpc_args.connect()?;
    let mut hot = vault_cli::hot::HotWallet::open_or_create(data_dir)?;
    hot.sync(&rpc.client)?;
    let (transaction, fee_sats) = hot.build_payment(address.script_pubkey(), amount_sats)?;
    let txid = rpc.client.send_raw_transaction(&transaction)?;
    println!("Hot-wallet payment broadcast: {txid}");
    println!("Destination: {address}");
    println!("Amount: {amount_sats} sats");
    println!("Fee: {fee_sats} sats (1 sat/vB)");
    Ok(())
}

fn print_active_policy(data_dir: &Path) -> Result<()> {
    let config = vault_cli::state::load_config(data_dir)?;
    println!("Cold storage descriptor: {}", config.vault_descriptor);
    println!("Vault address: {}", config.vault_address);
    println!(
        "Phone recovery: {} blocks (~14 months)",
        format_number(u64::from(config.phone_recovery_blocks))
    );
    println!(
        "HWW recovery:   {} blocks (~15 months)",
        format_number(u64::from(config.hww_recovery_blocks))
    );
    if config.monthly_limit_sats == 0 {
        println!("Monthly spending: disabled");
    } else {
        println!("Monthly limit: {} sats", config.monthly_limit_sats);
        if let Ok(schedule) = vault_cli::ceremony::load_schedule(data_dir) {
            println!(
                "Presigned monthly transaction pairs: {}",
                schedule.entries.len()
            );
        }
    }
    Ok(())
}

fn print_status(data_dir: &Path, rpc_args: &RpcArgs) -> Result<()> {
    let config = vault_cli::state::load_config(data_dir)?;
    let rpc = rpc_args.connect()?;
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
    println!(
        "Monthly spending: {}",
        if config.monthly_limit_sats == 0 {
            "disabled".to_owned()
        } else {
            format!("{} sats", config.monthly_limit_sats)
        }
    );
    if let Some(oldest) = utxos.first() {
        println!("Oldest confirmation height: {}", oldest.confirmation_height);
        print_recovery_activation(
            "Phone",
            oldest.confirmation_height + u64::from(config.phone_recovery_blocks),
            info.blocks + 1,
            info.median_time,
        );
        print_recovery_activation(
            "HWW",
            oldest.confirmation_height + u64::from(config.hww_recovery_blocks),
            info.blocks + 1,
            info.median_time,
        );
    }
    Ok(())
}

fn run_node(command: NodeCommand, rpc_args: &RpcArgs) -> Result<()> {
    let rpc = rpc_args.connect()?;
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
            let address = regtest_address(&address)?;
            let hashes = rpc.mine(blocks, &address)?;
            println!("Mined {} blocks", hashes.len());
            if let Some(hash) = hashes.last() {
                println!("New tip: {hash}");
            }
        }
    }
    Ok(())
}

fn print_manifest(manifest: &vault_cli::ceremony::BatchManifest, stderr: bool) -> Result<()> {
    let mut output: Box<dyn Write> = if stderr {
        Box::new(io::stderr().lock())
    } else {
        Box::new(io::stdout().lock())
    };
    writeln!(output, "PHONE POLICY PROPOSAL")?;
    writeln!(
        output,
        "Cold storage descriptor: {}",
        manifest.vault_descriptor
    )?;
    writeln!(output, "Vault address: {}", manifest.vault_address)?;
    if manifest.monthly_limit_sats == 0 {
        writeln!(output, "Monthly spending: disabled")?;
    } else {
        writeln!(
            output,
            "Monthly limit: {} sats",
            manifest.monthly_limit_sats
        )?;
    }
    writeln!(output, "Fee rate: {} sat/vB", manifest.fee_rate_sat_vb)?;
    writeln!(output, "Total input: {} sats", manifest.total_input_sats)?;
    writeln!(output, "Monthly pairs: {}", manifest.chunk_count)?;
    if manifest.chunk_count < vault_cli::MONTHS_PER_ROLLOVER && manifest.monthly_limit_sats > 0 {
        writeln!(
            output,
            "WARNING: balance funds only {} of {} monthly allowances",
            manifest.chunk_count,
            vault_cli::MONTHS_PER_ROLLOVER
        )?;
    }
    writeln!(output, "Rollover txid: {}", manifest.rollover.unsigned_txid)?;
    writeln!(output, "Rollover fee: {} sats", manifest.rollover.fee_sats)?;
    writeln!(
        output,
        "Phone signed PSBTs: {}",
        1 + manifest.chunk_count * 2
    )?;
    Ok(())
}

fn report_sweep(
    package: &vault_cli::recovery::CooperativeSweepPackage,
    stderr: bool,
) -> Result<()> {
    let mut output: Box<dyn Write> = if stderr {
        Box::new(io::stderr().lock())
    } else {
        Box::new(io::stdout().lock())
    };
    writeln!(output, "COOPERATIVE VAULT SWEEP")?;
    writeln!(output, "Destination: {}", package.destination)?;
    writeln!(output, "Inputs: {}", package.input_count)?;
    writeln!(output, "Sent: {} sats", package.sent_sats)?;
    writeln!(output, "Fee: {} sats (1 sat/vB)", package.fee_sats)?;
    writeln!(output, "Phone signed: {}", package.phone_approved)?;
    if package.hww_approved {
        writeln!(output, "HWW signed: true")?;
    }
    Ok(())
}

fn report_rotation(
    package: &vault_cli::recovery::PhoneRotationPackage,
    stderr: bool,
) -> Result<()> {
    let mut output: Box<dyn Write> = if stderr {
        Box::new(io::stderr().lock())
    } else {
        Box::new(io::stdout().lock())
    };
    writeln!(output, "PHONE-KEY ROTATION")?;
    writeln!(
        output,
        "New phone vault key: {}",
        package.new_phone_vault_pubkey
    )?;
    writeln!(
        output,
        "New cold storage descriptor: {}",
        package.new_vault_descriptor
    )?;
    writeln!(output, "New vault address: {}", package.new_vault_address)?;
    writeln!(output, "Inputs: {}", package.sweep.input_count)?;
    writeln!(output, "Sent: {} sats", package.sweep.sent_sats)?;
    writeln!(output, "Fee: {} sats (1 sat/vB)", package.sweep.fee_sats)?;
    if let Some(policy) = &package.renewed_policy {
        writeln!(
            output,
            "Monthly policy preserved: {} sats",
            policy.manifest.monthly_limit_sats
        )?;
        writeln!(
            output,
            "Renewed monthly pairs: {}",
            policy.manifest.chunk_count
        )?;
        writeln!(
            output,
            "Renewed policy PSBTs: {}",
            1 + policy.manifest.chunk_count * 2
        )?;
    } else {
        writeln!(output, "Monthly spending remains disabled")?;
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

fn print_sweep_result(label: &str, result: &vault_cli::recovery::SweepResult) {
    println!("{label}: {}", result.txid);
    println!("Inputs: {}", result.input_count);
    println!("Sent: {} sats", result.sent_sats);
    println!("Fee: {} sats (1 sat/vB)", result.fee_sats);
}

fn reset_workspace(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to reset workspace {}", path.display()))?;
    }
    Ok(())
}

fn read_artifact<T: DeserializeOwned>(path: &Path) -> Result<T> {
    if path == Path::new("-") {
        serde_json::from_reader(io::stdin().lock())
            .context("failed to read JSON artifact from stdin")
    } else {
        vault_cli::state::read_json(path)
    }
}

fn write_artifact<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if path == Path::new("-") {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, value)?;
        writeln!(stdout)?;
        Ok(())
    } else {
        vault_cli::state::write_json(path, value)
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

fn regtest_address(text: &str) -> Result<bitcoin::Address> {
    use std::str::FromStr;
    bitcoin::Address::from_str(text)?
        .require_network(bitcoin::Network::Regtest)
        .map_err(Into::into)
}

fn print_recovery_activation(label: &str, valid_height: u64, next_height: u64, median_time: u64) {
    if next_height >= valid_height {
        println!("{label} recovery: ACTIVE (valid next-block height {valid_height})");
    } else {
        let remaining = valid_height - next_height;
        let estimated_timestamp = median_time.saturating_add(remaining.saturating_mul(600));
        let estimated = i64::try_from(estimated_timestamp)
            .ok()
            .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
            .map(|date| date.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "outside displayable range".to_owned());
        println!(
            "{label} recovery: valid next-block height {valid_height} ({remaining} blocks remaining; estimated {estimated} at 10-minute blocks)"
        );
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
