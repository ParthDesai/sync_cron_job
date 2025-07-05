use crate::{
    errors::{Result, SyncCronError},
    solana_client::TransactionResult,
};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sled::Db;
use std::path::Path;
use uuid;

#[derive(Debug, Clone)]
pub struct Database {
    db: Db,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionRecord {
    pub id: String,
    pub tx_result: TransactionResult,
    pub sent_at: DateTime<Utc>,
    pub status: String, // Always "confirmed" since we wait for confirmation
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DailyStats {
    pub date: String,
    pub transactions_sent: i64,
    pub successful_transactions: i64,
    pub failed_transactions: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserKeyRecord {
    pub id: String,
    pub pubkey: String,
    pub private_key: String, // Base58 encoded
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
}

impl UserKeyRecord {
    pub fn new(pubkey: String, private_key: String, expires_at: DateTime<Utc>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            pubkey,
            private_key,
            created_at: now,
            expires_at,
            last_used: None,
        }
    }

    pub fn is_expired(&self) -> bool {
        Utc::now() > self.expires_at
    }

    pub fn mark_used(&mut self) {
        self.last_used = Some(Utc::now());
    }
}

impl Database {
    pub async fn new(database_path: &str) -> Result<Self> {
        // If path ends with .db, treat it as a file and use parent directory
        let final_path = if database_path.ends_with(".db") {
            Path::new(database_path)
                .parent()
                .unwrap_or(Path::new("."))
                .join("kv_store")
        } else {
            Path::new(database_path).to_path_buf()
        };

        let db = sled::open(&final_path)
            .map_err(|e| SyncCronError::Config(format!("Failed to open KV database: {}", e)))?;

        Ok(Self { db })
    }

    pub async fn migrate(&self) -> Result<()> {
        // No migrations needed for KV store - it's schema-less
        log::info!("💾 KV database initialized (no migrations needed)");
        Ok(())
    }

    pub async fn get_daily_transaction_count(&self) -> Result<i64> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let key = format!("daily_count:{}", today);

        match self.db.get(&key)? {
            Some(bytes) => {
                let count_str = String::from_utf8(bytes.to_vec())
                    .map_err(|e| SyncCronError::Config(format!("Invalid count format: {}", e)))?;
                Ok(count_str.parse::<i64>().unwrap_or(0))
            }
            None => Ok(0),
        }
    }

    pub async fn get_daily_target(&self, date: &str) -> Result<Option<u32>> {
        let key = format!("daily_target:{}", date);

        match self.db.get(&key)? {
            Some(bytes) => {
                let target_str = String::from_utf8(bytes.to_vec())
                    .map_err(|e| SyncCronError::Config(format!("Invalid target format: {}", e)))?;
                Ok(Some(target_str.parse::<u32>().unwrap_or(0)))
            }
            None => Ok(None),
        }
    }

    pub async fn set_daily_target(&self, date: &str, target: u32) -> Result<()> {
        let key = format!("daily_target:{}", date);
        self.db.insert(&key, target.to_string().as_bytes())?;
        self.db.flush_async().await?;
        Ok(())
    }

    pub async fn get_or_generate_daily_target(
        &self,
        date: &str,
        min_target: u32,
        max_target: u32,
    ) -> Result<u32> {
        // Try to get existing target for this date
        if let Some(target) = self.get_daily_target(date).await? {
            return Ok(target);
        }

        // Generate new random target within the range
        let target = if min_target == max_target {
            min_target
        } else {
            rand::thread_rng().gen_range(min_target..=max_target)
        };

        // Store the target for this date
        self.set_daily_target(date, target).await?;

        log::info!(
            "🎯 Generated new daily target for {}: {} (range: {}-{})",
            date,
            target,
            min_target,
            max_target
        );

        Ok(target)
    }

    pub async fn insert_transaction(&self, tx_result: TransactionResult) -> Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();

        let record = TransactionRecord {
            id: id.clone(),
            tx_result: tx_result.clone(),
            sent_at: now,
            status: "confirmed".to_string(),
        };

        // Store transaction record
        let tx_key = format!("tx:{}", tx_result.signatures[0]);
        let serialized = serde_json::to_vec(&record)?;
        self.db.insert(&tx_key, serialized)?;

        // Update daily count
        let today = now.format("%Y-%m-%d").to_string();
        let count_key = format!("daily_count:{}", today);
        let current_count = self.get_daily_transaction_count().await?;
        self.db
            .insert(&count_key, (current_count + 1).to_string().as_bytes())?;

        self.db.flush_async().await?;
        Ok(())
    }

    pub async fn get_transaction_by_hash(
        &self,
        tx_hash: &str,
    ) -> Result<Option<TransactionRecord>> {
        let tx_key = format!("tx:{}", tx_hash);

        match self.db.get(&tx_key)? {
            Some(bytes) => {
                let record: TransactionRecord = serde_json::from_slice(&bytes)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    pub async fn get_daily_stats(&self, date: &str) -> Result<DailyStats> {
        let mut total_transactions = 0i64;
        let mut successful_transactions = 0i64;
        let mut failed_transactions = 0i64;

        // Scan all transactions for the given date
        for result in self.db.scan_prefix("tx:") {
            let (_, record_bytes) = result?;
            if let Ok(record) = serde_json::from_slice::<TransactionRecord>(&record_bytes) {
                let tx_date = record.sent_at.format("%Y-%m-%d").to_string();
                if tx_date == date {
                    total_transactions += 1;
                    match record.status.as_str() {
                        "confirmed" => successful_transactions += 1,
                        "failed" => failed_transactions += 1,
                        _ => {}
                    }
                }
            }
        }

        Ok(DailyStats {
            date: date.to_string(),
            transactions_sent: total_transactions,
            successful_transactions,
            failed_transactions,
        })
    }

    pub async fn cleanup_old_transactions(&self, days_old: i64) -> Result<u64> {
        let cutoff_date = Utc::now() - chrono::Duration::days(days_old);
        let mut deleted_count = 0u64;

        let mut keys_to_delete = Vec::new();

        // Find old transactions
        for result in self.db.scan_prefix("tx:") {
            let (key, record_bytes) = result?;
            if let Ok(record) = serde_json::from_slice::<TransactionRecord>(&record_bytes) {
                if record.sent_at < cutoff_date {
                    keys_to_delete.push(key);
                }
            }
        }

        // Delete old transactions
        for tx_key in keys_to_delete {
            self.db.remove(&tx_key)?;
            deleted_count += 1;
        }

        if deleted_count > 0 {
            self.db.flush_async().await?;
        }

        Ok(deleted_count)
    }

    pub async fn get_all_transactions(&self) -> Result<Vec<TransactionRecord>> {
        let mut transactions = Vec::new();

        for result in self.db.scan_prefix("tx:") {
            let (_, record_bytes) = result?;
            if let Ok(record) = serde_json::from_slice::<TransactionRecord>(&record_bytes) {
                transactions.push(record);
            }
        }

        // Sort by sent_at timestamp (newest first)
        transactions.sort_by(|a, b| b.sent_at.cmp(&a.sent_at));

        Ok(transactions)
    }

    // User keypair pool management methods

    pub async fn get_user_key_pool(&self) -> Result<Vec<UserKeyRecord>> {
        let mut user_keys = Vec::new();

        for result in self.db.scan_prefix("user_key:") {
            let (_, record_bytes) = result?;
            if let Ok(record) = serde_json::from_slice::<UserKeyRecord>(&record_bytes) {
                user_keys.push(record);
            }
        }

        // Sort by creation time (oldest first)
        user_keys.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        Ok(user_keys)
    }

    pub async fn get_random_user_key(
        &self,
        min_expiry_seconds: u32,
        max_expiry_seconds: u32,
    ) -> Result<UserKeyRecord> {
        let user_keys = self.get_user_key_pool().await?;

        if user_keys.is_empty() {
            return Err(SyncCronError::Config(
                "No user keys available in pool".to_string(),
            ));
        }

        // Randomly select a key from the pool
        let random_index = rand::thread_rng().gen_range(0..user_keys.len());
        let selected_key = &user_keys[random_index];

        // Check if the selected key is expired
        if selected_key.is_expired() {
            log::info!(
                "⏰ Selected user key {} is expired, replacing with new key",
                selected_key.pubkey
            );

            // Remove the expired key
            self.remove_user_key(&selected_key.id).await?;

            // Create a new key to replace it
            let new_key = self
                .create_new_user_key(min_expiry_seconds, max_expiry_seconds)
                .await?;
            self.add_user_key(new_key.clone()).await?;

            return Ok(new_key);
        }

        // Key is not expired, mark it as used and return
        let mut key_to_use = selected_key.clone();
        key_to_use.mark_used();
        self.update_user_key(&key_to_use).await?;

        log::info!(
            "🔑 Using existing user key: {} (expires: {})",
            key_to_use.pubkey,
            key_to_use.expires_at
        );

        Ok(key_to_use)
    }

    pub async fn ensure_pool_size(
        &self,
        target_size: u32,
        min_expiry_seconds: u32,
        max_expiry_seconds: u32,
    ) -> Result<()> {
        let current_keys = self.get_user_key_pool().await?;
        let current_size = current_keys.len();

        log::info!(
            "📊 Current user key pool size: {}, target size: {}",
            current_size,
            target_size
        );

        if current_size < target_size as usize {
            let keys_to_create = target_size as usize - current_size;
            log::info!(
                "🔧 Creating {} new user keys to reach target pool size",
                keys_to_create
            );

            for i in 0..keys_to_create {
                let new_key = self
                    .create_new_user_key(min_expiry_seconds, max_expiry_seconds)
                    .await?;
                self.add_user_key(new_key).await?;
                log::info!("✅ Created user key {}/{}", i + 1, keys_to_create);
            }
        } else if current_size > target_size as usize {
            let keys_to_remove = current_size - target_size as usize;
            log::info!(
                "🗑️ Removing {} excess user keys to reach target pool size",
                keys_to_remove
            );

            // Remove oldest keys first
            for i in 0..keys_to_remove {
                if let Some(key_to_remove) = current_keys.get(i) {
                    self.remove_user_key(&key_to_remove.id).await?;
                    log::info!("❌ Removed excess user key: {}", key_to_remove.pubkey);
                }
            }
        } else {
            log::info!("✅ User key pool is already at target size");
        }

        Ok(())
    }

    pub async fn add_user_key(&self, user_key: UserKeyRecord) -> Result<()> {
        let key = format!("user_key:{}", user_key.id);
        let serialized = serde_json::to_vec(&user_key)?;
        self.db.insert(&key, serialized)?;
        self.db.flush_async().await?;

        log::info!(
            "➕ Added user key to pool: {} (expires: {})",
            user_key.pubkey,
            user_key.expires_at
        );

        Ok(())
    }

    pub async fn update_user_key(&self, user_key: &UserKeyRecord) -> Result<()> {
        let key = format!("user_key:{}", user_key.id);
        let serialized = serde_json::to_vec(user_key)?;
        self.db.insert(&key, serialized)?;
        self.db.flush_async().await?;
        Ok(())
    }

    pub async fn remove_user_key(&self, user_key_id: &str) -> Result<()> {
        let key = format!("user_key:{}", user_key_id);
        self.db.remove(&key)?;
        self.db.flush_async().await?;
        Ok(())
    }

    pub async fn cleanup_expired_user_keys(&self) -> Result<u64> {
        let user_keys = self.get_user_key_pool().await?;
        let mut deleted_count = 0u64;

        for key in user_keys {
            if key.is_expired() {
                if let Err(e) = self.remove_user_key(&key.id).await {
                    log::warn!("⚠️ Failed to remove expired user key {}: {}", key.id, e);
                } else {
                    deleted_count += 1;
                    log::info!("🗑️ Removed expired user key: {}", key.pubkey);
                }
            }
        }

        Ok(deleted_count)
    }

    pub async fn get_or_create_user_key(
        &self,
        pool_size: u32,
        min_expiry_seconds: u32,
        max_expiry_seconds: u32,
    ) -> Result<UserKeyRecord> {
        // Ensure the pool is at the target size first
        self.ensure_pool_size(pool_size, min_expiry_seconds, max_expiry_seconds)
            .await?;

        // Get a random key from the pool (this handles expiry replacement automatically)
        self.get_random_user_key(min_expiry_seconds, max_expiry_seconds)
            .await
    }

    async fn create_new_user_key(
        &self,
        min_expiry_seconds: u32,
        max_expiry_seconds: u32,
    ) -> Result<UserKeyRecord> {
        use anchor_client::solana_sdk::signature::{Keypair, Signer};

        // Generate a new keypair
        let keypair = Keypair::new();
        let pubkey = keypair.pubkey().to_string();

        // Encode the private key as base58
        let private_key = bs58::encode(keypair.to_bytes()).into_string();

        // Generate random expiry time
        let expiry_seconds = if min_expiry_seconds == max_expiry_seconds {
            min_expiry_seconds
        } else {
            rand::thread_rng().gen_range(min_expiry_seconds..=max_expiry_seconds)
        };

        let expires_at = Utc::now() + chrono::Duration::seconds(expiry_seconds as i64);

        let user_key = UserKeyRecord::new(pubkey, private_key, expires_at);

        log::info!(
            "🔑 Created new user key: {} (expires in {} seconds)",
            user_key.pubkey,
            expiry_seconds
        );

        Ok(user_key)
    }

    pub async fn get_user_key_pool_stats(&self) -> Result<(usize, usize, usize)> {
        let user_keys = self.get_user_key_pool().await?;
        let total_keys = user_keys.len();
        let expired_keys = user_keys.iter().filter(|k| k.is_expired()).count();
        let available_keys = total_keys - expired_keys;

        Ok((total_keys, available_keys, expired_keys))
    }
}
