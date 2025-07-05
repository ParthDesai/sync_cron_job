mod config;
mod database;
mod errors;
mod pinata_client;
mod solana_client;
mod transaction_manager;

use config::AppConfig;
use database::Database;
use errors::Result;
use pinata_client::PinataClient;
use solana_client::SolanaClient;
use transaction_manager::TransactionManager;

use chrono::Utc;
use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    log::info!("🚀 Starting Solana Sync Cron Job...");

    // Load configuration
    let config = AppConfig::load()?;
    log::info!("✅ Configuration loaded successfully");

    // Initialize database
    let database = Database::new(&config.database_path).await?;
    database.migrate().await?;
    log::info!("💾 KV database initialized");

    // Initialize user key pool
    log::info!("🔄 Initializing user key pool...");
    database
        .ensure_pool_size(
            config.user_key_pool_size,
            config.min_user_key_expiry_seconds,
            config.max_user_key_expiry_seconds,
        )
        .await?;
    log::info!(
        "🔑 User key pool initialized with {} keys",
        config.user_key_pool_size
    );

    let pinata_client = PinataClient::new(
        config.pinata_config.jwt_token.clone(),
        config.pinata_config.gateway_url.clone(),
    )
    .expect("Pinata client must initialize");
    log::info!("📁 Pinata client initialized");

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

    // Create transaction manager
    let transaction_manager = Arc::new(TransactionManager::new(
        database,
        solana_client,
        config.clone(),
    ));
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
