use crate::errors::{Result, SyncCronError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub solana_rpc_url: String,
    pub database_path: String,
    pub min_daily_transactions: u32,
    pub max_daily_transactions: u32,
    pub min_user_key_expiry_seconds: u32,
    pub max_user_key_expiry_seconds: u32,
    pub user_key_pool_size: u32,
    pub high_rating_percentage: f64,
    pub cron_schedule_in_minutes: u32,
    pub solana_config: SolanaConfig,
    pub pinata_config: PinataConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinataConfig {
    pub jwt_token: String,
    pub gateway_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaConfig {
    pub keypair_file: String, // Base58 encoded private key
    pub program_id: String,
    pub commitment: String,
    pub token_mint_address: String,
    pub agents: Vec<String>, // Array of agent private keys (base58 encoded)
    pub categories_supported: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    pub pubkey: String,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        if let Ok(config) = Self::load_from_file() {
            log::info!("📄 Configuration loaded from file");
            config.validate_categories()?;
            log::info!("✅ Category validation passed");
            return Ok(config);
        }

        log::info!("🔄 Loading configuration from environment variables");
        let config = Self::load_from_env()?;
        config.validate_categories()?;
        log::info!("✅ Category validation passed");
        Ok(config)
    }

    /// Validate that all categories fit within the size constraints defined in sync_contract
    fn validate_categories(&self) -> Result<()> {
        // Get the size limits from sync_contract constants
        let primary_category_size = sync_contract::types::PRIMARY_CATEGORY_SIZE;
        let secondary_category_size = sync_contract::types::SECONDARY_CATEGORY_SIZE;

        log::info!("🔍 Validating {} primary categories against size limits (primary: {} bytes, secondary: {} bytes)", 
            self.solana_config.categories_supported.len(),
            primary_category_size,
            secondary_category_size
        );

        for (primary_category, secondary_categories) in &self.solana_config.categories_supported {
            // Check primary category size
            if primary_category.as_bytes().len() > primary_category_size {
                return Err(SyncCronError::Config(format!(
                    "Primary category '{}' exceeds maximum size of {} bytes (actual: {} bytes)",
                    primary_category,
                    primary_category_size,
                    primary_category.as_bytes().len()
                )));
            }

            // Check each secondary category size
            for secondary_category in secondary_categories {
                if secondary_category.as_bytes().len() > secondary_category_size {
                    return Err(SyncCronError::Config(format!(
                        "Secondary category '{}' exceeds maximum size of {} bytes (actual: {} bytes)",
                        secondary_category,
                        secondary_category_size,
                        secondary_category.as_bytes().len()
                    )));
                }
            }
        }

        Ok(())
    }

    /// Get a random primary category and one of its secondary categories
    pub fn get_random_categories(&self) -> Result<(String, String)> {
        if self.solana_config.categories_supported.is_empty() {
            return Err(SyncCronError::Config(
                "No categories configured in categories_supported".to_string(),
            ));
        }

        use rand::Rng;
        let mut rng = rand::thread_rng();

        // Get all primary categories
        let primary_categories: Vec<&String> =
            self.solana_config.categories_supported.keys().collect();

        // Select a random primary category
        let primary_category = primary_categories[rng.gen_range(0..primary_categories.len())];

        // Get secondary categories for the selected primary category
        let secondary_categories = self
            .solana_config
            .categories_supported
            .get(primary_category)
            .unwrap();

        if secondary_categories.is_empty() {
            return Err(SyncCronError::Config(format!(
                "No secondary categories found for primary category '{}'",
                primary_category
            )));
        }

        // Select a random secondary category
        let secondary_category =
            &secondary_categories[rng.gen_range(0..secondary_categories.len())];

        log::debug!(
            "🎯 Selected random categories: Primary='{}', Secondary='{}'",
            primary_category,
            secondary_category
        );

        Ok((primary_category.clone(), secondary_category.clone()))
    }

    fn load_from_file() -> Result<Self> {
        let config_path = env::var("CONFIG_PATH").unwrap_or_else(|_| "config.json".to_string());

        if !Path::new(&config_path).exists() {
            return Err(SyncCronError::Config(format!(
                "Config file not found: {}",
                config_path
            )));
        }

        log::debug!("📖 Reading config file: {}", config_path);
        let content = fs::read_to_string(&config_path)?;
        let config: AppConfig = serde_json::from_str(&content)?;
        log::debug!("✅ Config file parsed successfully");
        Ok(config)
    }

    fn load_from_env() -> Result<Self> {
        let solana_rpc_url = env::var("SOLANA_RPC_URL")
            .unwrap_or_else(|_| "https://api.mainnet-beta.solana.com".to_string());

        let database_path = env::var("DATABASE_PATH")
            .or_else(|_| env::var("DATABASE_URL"))
            .unwrap_or_else(|_| "./kv_store".to_string());

        let min_daily_transactions = env::var("MIN_DAILY_TRANSACTIONS")
            .unwrap_or_else(|_| "100".to_string())
            .parse::<u32>()
            .map_err(|e| SyncCronError::Config(format!("Invalid min daily transactions: {}", e)))?;

        let max_daily_transactions = env::var("MAX_DAILY_TRANSACTIONS")
            .unwrap_or_else(|_| "100".to_string())
            .parse::<u32>()
            .map_err(|e| SyncCronError::Config(format!("Invalid max daily transactions: {}", e)))?;

        let min_user_key_expiry_seconds = env::var("MIN_USER_KEY_EXPIRY_SECONDS")
            .unwrap_or_else(|_| "3600".to_string())
            .parse::<u32>()
            .map_err(|e| {
                SyncCronError::Config(format!("Invalid min user key expiry seconds: {}", e))
            })?;

        let max_user_key_expiry_seconds = env::var("MAX_USER_KEY_EXPIRY_SECONDS")
            .unwrap_or_else(|_| "86400".to_string())
            .parse::<u32>()
            .map_err(|e| {
                SyncCronError::Config(format!("Invalid max user key expiry seconds: {}", e))
            })?;

        let user_key_pool_size = env::var("USER_KEY_POOL_SIZE")
            .unwrap_or_else(|_| "100".to_string())
            .parse::<u32>()
            .map_err(|e| SyncCronError::Config(format!("Invalid user key pool size: {}", e)))?;

        let high_rating_percentage = env::var("HIGH_RATING_PERCENTAGE")
            .unwrap_or_else(|_| "0.8".to_string())
            .parse::<f64>()
            .map_err(|e| SyncCronError::Config(format!("Invalid high rating percentage: {}", e)))?;

        let cron_schedule_in_minutes = env::var("CRON_SCHEDULE_IN_MINUTES")
            .unwrap_or_else(|_| "15".to_string())
            .parse::<u32>()
            .map_err(|e| {
                SyncCronError::Config(format!("Invalid cron schedule in minutes: {}", e))
            })?;

        let keypair_file = env::var("KEYPAIR_FILE")
            .unwrap_or_else(|_| "your_keypair_private_key_here".to_string());

        let program_id = env::var("PROGRAM_ID")
            .unwrap_or_else(|_| "11111111111111111111111111111112".to_string());

        let commitment = env::var("COMMITMENT").unwrap_or_else(|_| "confirmed".to_string());

        let token_mint_address = env::var("TOKEN_MINT_ADDRESS")
            .unwrap_or_else(|_| "11111111111111111111111111111112".to_string());

        let instruction_data = env::var("INSTRUCTION_DATA").unwrap_or_default();

        // Load agent private keys from environment (comma-separated)
        let agents = env::var("SOLANA_AGENTS")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.trim().to_string())
            .collect::<Vec<String>>();

        // Load Pinata config from environment if available
        let pinata_config = env::var("PINATA_JWT_TOKEN")
            .ok()
            .map(|jwt_token| PinataConfig {
                jwt_token,
                gateway_url: env::var("PINATA_GATEWAY_URL")
                    .unwrap_or_else(|_| "https://gateway.pinata.cloud".to_string()),
            })
            .expect("Pinata config must exist");

        // Create default categories if not provided via environment
        let mut categories_supported = HashMap::new();
        categories_supported.insert("general".to_string(), vec!["misc".to_string()]);
        log::debug!("🏷️ Created default categories: general -> misc");

        Ok(AppConfig {
            solana_rpc_url,
            database_path,
            min_daily_transactions,
            max_daily_transactions,
            min_user_key_expiry_seconds,
            max_user_key_expiry_seconds,
            user_key_pool_size,
            high_rating_percentage,
            cron_schedule_in_minutes,
            solana_config: SolanaConfig {
                keypair_file,
                program_id: program_id.clone(),
                commitment,
                token_mint_address,
                agents,
                categories_supported,
            },
            pinata_config,
        })
    }

    pub fn save_example() -> Result<()> {
        log::info!("📝 Creating example configuration...");

        let mut categories_supported = HashMap::new();
        categories_supported.insert(
            "technology".to_string(),
            vec![
                "ai".to_string(),
                "blockchain".to_string(),
                "software".to_string(),
            ],
        );
        categories_supported.insert(
            "science".to_string(),
            vec![
                "physics".to_string(),
                "chemistry".to_string(),
                "biology".to_string(),
            ],
        );
        categories_supported.insert(
            "business".to_string(),
            vec![
                "finance".to_string(),
                "marketing".to_string(),
                "strategy".to_string(),
            ],
        );

        log::debug!(
            "🏷️ Added {} example category groups",
            categories_supported.len()
        );

        let example_config = AppConfig {
            solana_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            database_path: "./kv_store".to_string(),
            min_daily_transactions: 100,
            max_daily_transactions: 100,
            min_user_key_expiry_seconds: 3600,
            max_user_key_expiry_seconds: 86400,
            user_key_pool_size: 100,
            high_rating_percentage: 0.8,
            cron_schedule_in_minutes: 15,
            solana_config: SolanaConfig {
                keypair_file: "your_keypair_file_here".to_string(),
                program_id: "11111111111111111111111111111112".to_string(),
                commitment: "confirmed".to_string(),
                token_mint_address: "your_token_mint_address_here".to_string(),
                agents: vec!["your_agent_private_key_here".to_string()],
                categories_supported,
            },
            pinata_config: PinataConfig {
                jwt_token: "your_pinata_jwt_token_here".to_string(),
                gateway_url: "https://gateway.pinata.cloud".to_string(),
            },
        };

        let json = serde_json::to_string_pretty(&example_config)?;
        fs::write("config.example.json", json)?;
        log::info!("✅ Example configuration saved to config.example.json");
        Ok(())
    }
}
