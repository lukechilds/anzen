use anyhow::Result;
use clap::{Args, Parser, Subcommand};
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
    }
    Ok(())
}
