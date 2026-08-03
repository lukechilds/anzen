use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "vault-cli", version, about)]
struct Cli {
    /// Directory containing simulated device, cloud, and wallet state.
    #[arg(long, default_value = ".vault-data", global = true)]
    data_dir: PathBuf,

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
    }
    Ok(())
}
