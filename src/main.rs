mod config;
mod database;
mod errors;
mod evm_client;
mod pinata_client;
mod solana_client;
mod solana_tx_fetcher;
mod transaction_manager;

use anchor_client::solana_sdk::signature::Signature;
use config::AppConfig;
use database::Database;
use database::ChainTarget;
use errors::Result;
use pinata_client::PinataClient;
use evm_client::EvmClient;
use solana_client::SolanaClient;
use transaction_manager::{ChainClient, TransactionManager};

use chrono::Utc;
use clap::{Arg, ArgMatches, Command};
use errors::SyncCronError;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};

use crate::solana_tx_fetcher::SolanaTxFetcher;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // Parse command line arguments
    let matches = Command::new("sync_cron")
        .version("0.1.0")
        .about("Solana Sync Cron Job and Transaction Monitor")
        .subcommand(
            Command::new("fetch-tx-data")
                .about("Fetch transaction data from blockchain for sync_contract")
                .arg(
                    Arg::new("before")
                        .long("before")
                        .help("Fetch transactions strictly before this signature")
                        .value_name("SIGNATURE"),
                )
                .arg(
                    Arg::new("until")
                        .long("until")
                        .help("Stop when this signature is reached")
                        .value_name("SIGNATURE"),
                )
                .arg(
                    Arg::new("output")
                        .long("output")
                        .short('o')
                        .help("Output file path (must not already exist)")
                        .value_name("FILE")
                        .required(true),
                ),
        )
        .get_matches();

    // Load configuration
    let config = AppConfig::load()?;
    log::info!("✅ Configuration loaded successfully");

    // Handle subcommands
    if let Some(sub_matches) = matches.subcommand_matches("fetch-tx-data") {
        return handle_fetch_tx_data(sub_matches, config).await;
    }

    log::info!(
        "🚀 Starting Sync Cron Job (target_chain={})...",
        config.target_chain
    );

    // Initialize database
    let database = Database::new(&config.database_path).await?;
    database.migrate().await?;
    log::info!("💾 KV database initialized");

    // Initialize user key pool (chain-aware)
    let (chain, evm_bridge_script_path) = if config.target_chain.to_lowercase() == "base" {
        (ChainTarget::Base, config.evm_config.as_ref().map(|c| c.bridge_script_path.as_str()))
    } else {
        (ChainTarget::Solana, None)
    };

    log::info!("🔄 Initializing user key pool for chain={:?}...", chain);
    database
        .ensure_pool_size_for_chain(
            chain,
            config.user_key_pool_size,
            config.min_user_key_expiry_seconds,
            config.max_user_key_expiry_seconds,
            evm_bridge_script_path,
        )
        .await?;
    log::info!("🔑 User key pool initialized with {} keys", config.user_key_pool_size);

    let pinata_client = PinataClient::new(
        config.pinata_config.jwt_token.clone(),
        config.pinata_config.gateway_url.clone(),
    )
    .expect("Pinata client must initialize");
    log::info!("📁 Pinata client initialized");

    let chain_client = if config.target_chain.to_lowercase() == "base" {
        let evm = config
            .evm_config
            .clone()
            .ok_or_else(|| errors::SyncCronError::Config("evm_config required for TARGET_CHAIN=base".into()))?;
        let bridge_path = evm.bridge_script_path.clone();
        let evm_client = EvmClient::new(
            evm,
            config.clone(),
            pinata_client,
            database.clone(),
            bridge_path,
        );
        log::info!("🌐 Base/EVM client initialized");
        ChainClient::Base(evm_client)
    } else {
        // Load agent keypairs from configuration
        let agents = SolanaClient::load_agents_from_private_keys(&config.solana_config.agents);
        log::info!("👥 Loaded {} agent keypairs", agents.len());

        // Initialize Solana client with loaded agents
        let solana_client = SolanaClient::new(
            &config.solana_rpc_url,
            config.solana_config.clone(),
            pinata_client,
            agents,
            database.clone(),
            config.clone(),
        );
        log::info!("🌐 Solana client initialized");
        ChainClient::Solana(solana_client)
    };

    // Create transaction manager
    let transaction_manager = Arc::new(TransactionManager::new(database, chain_client, config.clone()));
    // Create job scheduler
    let mut scheduler = JobScheduler::new().await?;

    // Create the random transaction job that runs every N minutes
    let tm_clone = Arc::clone(&transaction_manager);
    let cron_expression = format!("0 */{} * * * *", config.cron_schedule_in_minutes);
    let job = Job::new_async(cron_expression.as_str(), move |_uuid, _l| {
        let tm = Arc::clone(&tm_clone);
        Box::pin(async move {
            if let Err(e) = tm.process_random_transactions().await {
                log::error!("Error processing random transactions: {}", e);
            }
        })
    })?;

    scheduler.add(job).await?;

    // Get today's daily target if it exists
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let daily_target_info = match transaction_manager
        .get_database()
        .get_daily_target(&today)
        .await?
    {
        Some(target) => format!("{}", target),
        None => "TBD".to_string(),
    };

    log::info!("⏰ Random transaction scheduler started (every {} minutes, daily range: {}-{}, today's target: {})", 
        config.cron_schedule_in_minutes,
        config.min_daily_transactions,
        config.max_daily_transactions,
        daily_target_info
    );

    transaction_manager.get_daily_stats().await?;

    // Start the scheduler
    scheduler.start().await?;

    // Keep the application running
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl+c");

    log::info!("🛑 Shutting down...");
    scheduler.shutdown().await?;

    Ok(())
}

async fn handle_fetch_tx_data(matches: &ArgMatches, config: AppConfig) -> Result<()> {
    log::info!("🔍 Fetching transaction data for sync_contract...");

    let solana_tx_fetcher =
        SolanaTxFetcher::new(config.solana_rpc_url, config.solana_config.clone());

    // Parse optional signatures
    let before_signature = if let Some(sig_str) = matches.get_one::<String>("before") {
        match Signature::from_str(sig_str) {
            Ok(sig) => Some(sig),
            Err(_) => {
                return Err(SyncCronError::Config(
                    "Invalid --before signature provided".to_string(),
                ))
            }
        }
    } else {
        None
    };

    let until_signature = if let Some(sig_str) = matches.get_one::<String>("until") {
        match Signature::from_str(sig_str) {
            Ok(sig) => Some(sig),
            Err(_) => {
                return Err(SyncCronError::Config(
                    "Invalid --until signature provided".to_string(),
                ))
            }
        }
    } else {
        None
    };

    // Prepare output file path (must not exist). Create parent directories if needed
    let output_path_str = matches
        .get_one::<String>("output")
        .expect("--output is required by clap");
    let output_path = Path::new(output_path_str);

    if output_path.exists() {
        return Err(SyncCronError::Config(format!(
            "Output file already exists: {}",
            output_path_str
        )));
    }

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    let file = tokio::fs::File::create_new(output_path).await?;

    // Fetch and process transaction data
    solana_tx_fetcher
        .fetch_sync_contract_transactions(before_signature, until_signature, file)
        .await?;

    Ok(())
}
