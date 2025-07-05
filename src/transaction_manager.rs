use crate::config::AppConfig;
use crate::database::Database;
use crate::errors::Result;
use crate::solana_client::{SolanaClient, TransactionResult};
use chrono::{Timelike, Utc};
use rand::Rng;

pub struct TransactionManager {
    database: Database,
    solana_client: SolanaClient,
    config: AppConfig,
}

impl TransactionManager {
    pub fn new(database: Database, solana_client: SolanaClient, config: AppConfig) -> Self {
        Self {
            database,
            solana_client,
            config,
        }
    }

    /// Get access to the database for external queries
    pub fn get_database(&self) -> &Database {
        &self.database
    }

    /// New method for random transaction processing throughout the day
    pub async fn process_random_transactions(&self) -> Result<()> {
        log::info!("🔍 Checking for random transaction opportunity...");

        // Get today's date
        let today = Utc::now().format("%Y-%m-%d").to_string();

        // Get or generate daily target for today
        let daily_target = self
            .database
            .get_or_generate_daily_target(
                &today,
                self.config.min_daily_transactions,
                self.config.max_daily_transactions,
            )
            .await?;

        // Check current daily transaction count
        let current_count = self.database.get_daily_transaction_count().await?;
        log::info!(
            "📊 Current daily transaction count: {}/{}",
            current_count,
            daily_target
        );

        // Check if we've reached the daily target
        if current_count >= daily_target as i64 {
            log::info!(
                "✅ Daily transaction target ({}) already reached ({}). Skipping...",
                daily_target,
                current_count
            );
            return Ok(());
        }

        // Calculate remaining transactions for today
        let remaining_transactions = daily_target as i64 - current_count;

        // Calculate probability of sending a transaction this cycle
        // We want to distribute transactions randomly throughout the day
        let probability = self
            .calculate_transaction_probability(remaining_transactions)
            .await?;

        // Generate random number to decide if we should send a transaction
        let random_value: f64 = rand::thread_rng().gen();

        log::info!(
            "🎲 Transaction probability: {:.2}%, random value: {:.2}%",
            probability * 100.0,
            random_value * 100.0
        );

        if random_value < probability {
            log::info!("🎯 Randomly selected to send transaction now!");

            match self.send_single_transaction().await {
                Ok(tx_result) => {
                    log::info!(
                        "✅ Random transactions sent successfully: {}",
                        tx_result.signatures.join(", ")
                    );

                    if let Err(e) = self.database.insert_transaction(tx_result).await {
                        log::error!("❌ Failed to record transaction in database: {}", e);
                    }
                }
                Err(e) => {
                    log::error!("❌ Failed to send random transactions: {}", e);
                }
            }
        } else {
            log::info!("⏭️ Skipping transaction this cycle (random selection)");
        }

        // Update daily transaction count and log statistics
        let current_count = self.database.get_daily_transaction_count().await?;
        log::info!(
            "Daily transaction count: {}/{}",
            current_count,
            daily_target
        );

        // Cleanup old transactions (older than 30 days)
        self.cleanup_old_transactions().await?;

        // Log user key pool statistics
        if let Ok((total, available, expired)) = self.database.get_user_key_pool_stats().await {
            log::info!(
                "User key pool stats: {} total, {} available, {} expired",
                total,
                available,
                expired
            );
        }

        log::info!("Random transaction processing completed");
        Ok(())
    }

    /// Calculate the probability of sending a transaction in this cycle
    async fn calculate_transaction_probability(&self, remaining_transactions: i64) -> Result<f64> {
        if remaining_transactions <= 0 {
            return Ok(0.0);
        }

        // Get current time info
        let now = Utc::now();
        let current_hour = now.hour();
        let current_minute = now.minute();

        // Calculate how many cycles are left in the day based on the configurable interval
        let cycle_interval = self.config.cron_schedule_in_minutes;
        let minutes_left_in_day = (24 - current_hour) * 60 - current_minute;
        let cycles_left_in_day = (minutes_left_in_day / cycle_interval) as i64;

        if cycles_left_in_day <= 0 {
            // If it's very late in the day, send remaining transactions with higher probability
            return Ok(1.0);
        }

        // Base probability: evenly distribute remaining transactions across remaining cycles
        let base_probability = remaining_transactions as f64 / cycles_left_in_day as f64;

        // Apply time-based adjustments to make transactions more realistic
        let time_multiplier = self.get_time_based_multiplier(current_hour);
        let adjusted_probability = base_probability * time_multiplier;

        // Cap probability at 1.0 and ensure minimum randomness
        let final_probability = adjusted_probability.min(0.8).max(0.01);

        log::info!(
            "Probability calculation: {} remaining, {} cycles left ({} min intervals), base: {:.3}, time_mult: {:.2}, final: {:.3}",
            remaining_transactions, cycles_left_in_day, cycle_interval, base_probability, time_multiplier, final_probability
        );

        Ok(final_probability)
    }

    /// Get time-based multiplier to simulate realistic transaction patterns
    fn get_time_based_multiplier(&self, hour: u32) -> f64 {
        match hour {
            // Early morning (00:00-06:00) - lower activity
            0..=5 => 0.3,
            // Morning (06:00-09:00) - moderate activity
            6..=8 => 0.7,
            // Business hours (09:00-17:00) - higher activity
            9..=16 => 1.2,
            // Evening (17:00-21:00) - moderate activity
            17..=20 => 0.8,
            // Late night (21:00-24:00) - lower activity
            21..=23 => 0.5,
            _ => 1.0,
        }
    }

    /// Original method for backward compatibility and manual triggering
    pub async fn process_daily_transactions(&self) -> Result<()> {
        log::info!("🔄 Processing daily transactions...");

        // Get today's date
        let today = Utc::now().format("%Y-%m-%d").to_string();

        // Get or generate daily target for today
        let daily_target = self
            .database
            .get_or_generate_daily_target(
                &today,
                self.config.min_daily_transactions,
                self.config.max_daily_transactions,
            )
            .await?;

        // Check current daily transaction count
        let current_count = self.database.get_daily_transaction_count().await?;
        log::info!(
            "📊 Current daily transaction count: {}/{}",
            current_count,
            daily_target
        );

        // Check if we've reached the daily target
        if current_count >= daily_target as i64 {
            log::info!(
                "Daily transaction target ({}) already reached ({}). Skipping...",
                daily_target,
                current_count
            );
            return Ok(());
        }

        // Calculate how many transactions we can send
        let remaining_transactions = daily_target as i64 - current_count;
        let transactions_to_send = std::cmp::min(remaining_transactions, 1); // Send one at a time per cron run

        log::info!("Sending {} transaction(s)", transactions_to_send);

        // Send transactions
        for i in 0..transactions_to_send {
            match self.send_single_transaction().await {
                Ok(tx_result) => {
                    log::info!(
                        "Transaction {} sent successfully: {}",
                        i + 1,
                        tx_result.signatures.join(", ")
                    );

                    // Insert transaction records
                    if let Err(e) = self.database.insert_transaction(tx_result).await {
                        log::error!("Failed to record transaction in database: {}", e);
                    }
                }
                Err(e) => {
                    log::error!("Failed to send transaction {}: {}", i + 1, e);
                    // Continue with next transaction
                }
            }
        }

        // Update daily transaction count and log statistics
        let current_count = self.database.get_daily_transaction_count().await?;
        log::info!(
            "Daily transaction count: {}/{}",
            current_count,
            daily_target
        );

        // Cleanup old transactions (older than 30 days)
        self.cleanup_old_transactions().await?;

        log::info!("Daily transaction processing completed");
        Ok(())
    }

    async fn send_single_transaction(&self) -> Result<TransactionResult> {
        let result = self
            .solana_client
            .submit_transaction(&self.config.solana_config)
            .await?;
        Ok(result)
    }

    async fn cleanup_old_transactions(&self) -> Result<()> {
        match self.database.cleanup_old_transactions(30).await {
            Ok(deleted_count) => {
                if deleted_count > 0 {
                    log::info!("Cleaned up {} old transaction records", deleted_count);
                }
            }
            Err(e) => {
                log::warn!("Failed to cleanup old transactions: {}", e);
            }
        }
        Ok(())
    }

    pub async fn get_daily_stats(&self) -> Result<()> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let stats = self.database.get_daily_stats(&today).await?;

        log::info!(
            "Daily stats for {}: {} total, {} successful, {} failed",
            stats.date,
            stats.transactions_sent,
            stats.successful_transactions,
            stats.failed_transactions
        );

        Ok(())
    }

    pub async fn force_send_transaction(&self) -> Result<Vec<String>> {
        log::info!("Force sending transaction (ignoring daily limits)");
        let result = self.send_single_transaction().await?;
        let tx_hashes = result.signatures.clone();

        // Insert transaction records
        self.database.insert_transaction(result).await?;

        Ok(tx_hashes)
    }
}
