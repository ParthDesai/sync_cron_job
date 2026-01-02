use crate::errors::{Result, SyncCronError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Which chain to use for cron operations. Supported: "solana", "base"
    #[serde(default = "default_target_chain")]
    pub target_chain: String,
    pub solana_rpc_url: String,
    /// Base/EVM configuration (required when target_chain == "base")
    #[serde(default)]
    pub evm_config: Option<EvmConfig>,
    pub database_path: String,
    pub min_daily_transactions: u32,
    pub max_daily_transactions: u32,
    pub min_user_key_expiry_seconds: u32,
    pub max_user_key_expiry_seconds: u32,
    pub user_key_pool_size: u32,
    pub high_rating_percentage: f64,
    pub cron_schedule_in_minutes: u32,
    pub min_file_size_bytes: u32,
    pub max_file_size_bytes: u32,
    pub solana_config: SolanaConfig,
    pub pinata_config: PinataConfig,
}

fn default_target_chain() -> String {
    "solana".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvmConfig {
    pub rpc_url: String,
    pub sync_contract_proxy: String,
    /// Domain stored on-chain for each submission (e.g. "syncora.ai")
    #[serde(default = "default_evm_domain")]
    pub domain: String,
    /// Optional admin key used to fund wallets and allow agents.
    #[serde(default)]
    pub admin_private_key: Option<String>,
    /// Comma-separated list in env; stored as Vec in config.
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default = "default_min_user_balance_wei")]
    pub min_user_balance_wei: u128,
    #[serde(default = "default_min_agent_balance_wei")]
    pub min_agent_balance_wei: u128,
    #[serde(default = "default_funding_amount_wei")]
    pub funding_amount_wei: u128,
    /// Path to Node bridge script (`scripts/evm_bridge.js`)
    #[serde(default = "default_evm_bridge_script_path")]
    pub bridge_script_path: String,
}

fn default_min_user_balance_wei() -> u128 {
    10_000_000_000_000_000u128 // 0.01 ETH
}
fn default_min_agent_balance_wei() -> u128 {
    10_000_000_000_000_000u128 // 0.01 ETH
}
fn default_funding_amount_wei() -> u128 {
    50_000_000_000_000_000u128 // 0.05 ETH
}
fn default_evm_bridge_script_path() -> String {
    "scripts/evm_bridge.js".to_string()
}

fn default_evm_domain() -> String {
    "syncora.ai".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinataConfig {
    pub jwt_token: String,
    pub gateway_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryConfig {
    /// Relative weight for this category. Larger means more likely.
    /// If omitted (legacy configs), defaults to 1.0.
    #[serde(default = "default_probability")]
    pub probability: f64,

    /// New schema: pick a data type within the category using probability.
    /// If empty, we fall back to legacy behavior (secondary_categories + file_extensions).
    #[serde(default)]
    pub data_types: Vec<DataTypeConfig>,

    /// Legacy schema (kept for backward compatibility).
    #[serde(default)]
    pub secondary_categories: Vec<String>,

    /// Legacy schema (kept for backward compatibility).
    #[serde(default)]
    pub file_extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataTypeConfig {
    pub name: String,
    #[serde(default = "default_probability")]
    pub probability: f64,
    /// Each data type supports multiple (format, extension) pairs. Pick one uniformly.
    pub formats: Vec<FormatExtension>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatExtension {
    pub format: String,
    pub extension: String,
}

fn default_probability() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaConfig {
    pub keypair_file: String, // Base58 encoded private key
    pub program_id: String,
    pub commitment: String,
    pub token_mint_address: String,
    pub agents: Vec<String>, // Array of agent private keys (base58 encoded)
    pub categories_supported: HashMap<String, CategoryConfig>,
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

    /// Backward-compatible alias (older code/tests may call this).
    pub fn get_random_categories(&self) -> Result<(String, String, String)> {
        let (cat, dt, _fmt, ext) = self.select_file_profile()?;
        Ok((cat, dt, ext))
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

        for (primary_category, category_config) in &self.solana_config.categories_supported {
            // Check primary category size
            if primary_category.as_bytes().len() > primary_category_size {
                return Err(SyncCronError::Config(format!(
                    "Primary category '{}' exceeds maximum size of {} bytes (actual: {} bytes)",
                    primary_category,
                    primary_category_size,
                    primary_category.as_bytes().len()
                )));
            }

            if !category_config.probability.is_finite() || category_config.probability < 0.0 {
                return Err(SyncCronError::Config(format!(
                    "Category '{}' has invalid probability: {}",
                    primary_category, category_config.probability
                )));
            }

            // New schema validation (if present)
            if !category_config.data_types.is_empty() {
                for dt in &category_config.data_types {
                    if dt.name.as_bytes().len() > secondary_category_size {
                        return Err(SyncCronError::Config(format!(
                            "Data type '{}' (category '{}') exceeds max size {} bytes (actual: {} bytes)",
                            dt.name,
                            primary_category,
                            secondary_category_size,
                            dt.name.as_bytes().len()
                        )));
                    }
                    if !dt.probability.is_finite() || dt.probability < 0.0 {
                        return Err(SyncCronError::Config(format!(
                            "Data type '{}' (category '{}') has invalid probability: {}",
                            dt.name, primary_category, dt.probability
                        )));
                    }
                    if dt.formats.is_empty() {
                        return Err(SyncCronError::Config(format!(
                            "Data type '{}' (category '{}') must have at least one (format, extension) pair",
                            dt.name, primary_category
                        )));
                    }
                    for fe in &dt.formats {
                        if fe.format.trim().is_empty() || fe.extension.trim().is_empty() {
                            return Err(SyncCronError::Config(format!(
                                "Data type '{}' (category '{}') contains empty format/extension",
                                dt.name, primary_category
                            )));
                        }
                    }
                }
            } else {
                // Legacy schema validation
                for secondary_category in &category_config.secondary_categories {
                    if secondary_category.as_bytes().len() > secondary_category_size {
                        return Err(SyncCronError::Config(format!(
                            "Secondary category '{}' exceeds maximum size of {} bytes (actual: {} bytes)",
                            secondary_category,
                            secondary_category_size,
                            secondary_category.as_bytes().len()
                        )));
                    }
                }
                if category_config.file_extensions.is_empty() {
                    return Err(SyncCronError::Config(format!(
                        "No file extensions found for primary category '{}' (legacy config)",
                        primary_category
                    )));
                }
            }
        }

        Ok(())
    }

    /// Select (category, datatype, format, extension) using probabilities.
    /// - Category chosen by `category.probability`
    /// - Data type chosen by `datatype.probability` inside the selected category
    /// - (format, extension) chosen uniformly from the selected data type
    ///
    /// Backward compatible:
    /// - If `data_types` is empty, we pick a legacy `secondary_category` (or empty) and a legacy extension.
    /// - Legacy selection is uniform (no probabilities).
    pub fn select_file_profile(&self) -> Result<(String, String, String, String)> {
        if self.solana_config.categories_supported.is_empty() {
            return Err(SyncCronError::Config(
                "No categories configured in categories_supported".to_string(),
            ));
        }

        use rand::Rng;
        let mut rng = rand::thread_rng();

        // Weighted pick category
        let mut categories: Vec<(&String, &CategoryConfig)> =
            self.solana_config.categories_supported.iter().collect();
        categories.sort_by(|a, b| a.0.cmp(b.0)); // deterministic iteration for logs/tests

        let total_weight: f64 = categories.iter().map(|(_, c)| c.probability.max(0.0)).sum();
        if total_weight <= 0.0 {
            return Err(SyncCronError::Config(
                "All category probabilities are 0; cannot select category".to_string(),
            ));
        }

        let mut roll = rng.gen::<f64>() * total_weight;
        let mut picked = categories[0];
        for item in categories {
            let w = item.1.probability.max(0.0);
            if roll <= w {
                picked = item;
                break;
            }
            roll -= w;
        }
        let (category_name, category_config) = picked;

        // New schema path
        if !category_config.data_types.is_empty() {
            let dts = &category_config.data_types;
            let dt_total: f64 = dts.iter().map(|d| d.probability.max(0.0)).sum();
            if dt_total <= 0.0 {
                return Err(SyncCronError::Config(format!(
                    "All datatype probabilities are 0 for category '{}'",
                    category_name
                )));
            }
            let mut dt_roll = rng.gen::<f64>() * dt_total;
            let mut dt_picked = &dts[0];
            for dt in dts {
                let w = dt.probability.max(0.0);
                if dt_roll <= w {
                    dt_picked = dt;
                    break;
                }
                dt_roll -= w;
            }

            let fe = dt_picked.formats[rng.gen_range(0..dt_picked.formats.len())].clone();
            log::debug!(
                "🎯 Selected file profile: category='{}' datatype='{}' format='{}' ext='{}'",
                category_name,
                dt_picked.name,
                fe.format,
                fe.extension
            );
            return Ok((
                category_name.clone(),
                dt_picked.name.clone(),
                fe.format,
                fe.extension,
            ));
        }

        // Select a random secondary category or use empty string if none available
        let secondary_category = if category_config.secondary_categories.is_empty() {
            String::new() // Return empty string if no secondary categories
        } else {
            category_config.secondary_categories
                [rng.gen_range(0..category_config.secondary_categories.len())]
            .clone()
        };

        // Select a random file extension
        let file_extension = &category_config.file_extensions
            [rng.gen_range(0..category_config.file_extensions.len())];

        log::debug!(
            "🎯 Selected legacy profile: category='{}', secondary='{}', ext='{}'",
            category_name,
            if secondary_category.is_empty() {
                "<empty>"
            } else {
                &secondary_category
            },
            file_extension
        );

        // Legacy mapping:
        // - datatype = secondary_category (or "default")
        // - format = "raw"
        Ok((
            category_name.clone(),
            if secondary_category.is_empty() {
                "default".to_string()
            } else {
                secondary_category
            },
            "raw".to_string(),
            file_extension.clone(),
        ))
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
        let target_chain = env::var("TARGET_CHAIN").unwrap_or_else(|_| "solana".to_string());

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

        let min_file_size_bytes = env::var("MIN_FILE_SIZE_BYTES")
            .unwrap_or_else(|_| "512".to_string())
            .parse::<u32>()
            .map_err(|e| SyncCronError::Config(format!("Invalid min file size bytes: {}", e)))?;

        let max_file_size_bytes = env::var("MAX_FILE_SIZE_BYTES")
            .unwrap_or_else(|_| "4096".to_string())
            .parse::<u32>()
            .map_err(|e| SyncCronError::Config(format!("Invalid max file size bytes: {}", e)))?;

        // EVM/Base config (only required when TARGET_CHAIN=base)
        let evm_config = if target_chain.to_lowercase() == "base" {
            let rpc_url = env::var("EVM_RPC_URL")
                .or_else(|_| env::var("BASE_MAINNET_RPC_URL"))
                .or_else(|_| env::var("BASE_SEPOLIA_RPC_URL"))
                .map_err(|_| {
                    SyncCronError::Config(
                        "Missing EVM_RPC_URL (or BASE_MAINNET_RPC_URL / BASE_SEPOLIA_RPC_URL)".into(),
                    )
                })?;

            let sync_contract_proxy = env::var("EVM_SYNC_CONTRACT_PROXY")
                .map_err(|_| SyncCronError::Config("Missing EVM_SYNC_CONTRACT_PROXY".into()))?;

            let domain = env::var("EVM_DOMAIN").unwrap_or_else(|_| default_evm_domain());

            let admin_private_key = env::var("EVM_ADMIN_PRIVATE_KEY").ok();

            let agents = env::var("EVM_AGENTS")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .collect::<Vec<String>>();

            let min_user_balance_wei = env::var("EVM_MIN_USER_BALANCE_WEI")
                .ok()
                .map(|v| v.parse::<u128>())
                .transpose()
                .map_err(|e| SyncCronError::Config(format!("Invalid EVM_MIN_USER_BALANCE_WEI: {}", e)))?
                .unwrap_or_else(default_min_user_balance_wei);

            let min_agent_balance_wei = env::var("EVM_MIN_AGENT_BALANCE_WEI")
                .ok()
                .map(|v| v.parse::<u128>())
                .transpose()
                .map_err(|e| SyncCronError::Config(format!("Invalid EVM_MIN_AGENT_BALANCE_WEI: {}", e)))?
                .unwrap_or_else(default_min_agent_balance_wei);

            let funding_amount_wei = env::var("EVM_FUNDING_AMOUNT_WEI")
                .ok()
                .map(|v| v.parse::<u128>())
                .transpose()
                .map_err(|e| SyncCronError::Config(format!("Invalid EVM_FUNDING_AMOUNT_WEI: {}", e)))?
                .unwrap_or_else(default_funding_amount_wei);

            let bridge_script_path = env::var("EVM_BRIDGE_SCRIPT_PATH")
                .unwrap_or_else(|_| default_evm_bridge_script_path());

            Some(EvmConfig {
                rpc_url,
                sync_contract_proxy,
                domain,
                admin_private_key,
                agents,
                min_user_balance_wei,
                min_agent_balance_wei,
                funding_amount_wei,
                bridge_script_path,
            })
        } else {
            None
        };

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
        categories_supported.insert(
            "general".to_string(),
            CategoryConfig {
                probability: 1.0,
                data_types: vec![],
                secondary_categories: vec!["misc".to_string()],
                file_extensions: vec!["txt".to_string()],
            },
        );
        log::debug!("🏷️ Created default categories: general -> misc (txt)");

        Ok(AppConfig {
            target_chain,
            solana_rpc_url,
            evm_config,
            database_path,
            min_daily_transactions,
            max_daily_transactions,
            min_user_key_expiry_seconds,
            max_user_key_expiry_seconds,
            user_key_pool_size,
            high_rating_percentage,
            cron_schedule_in_minutes,
            min_file_size_bytes,
            max_file_size_bytes,
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
            CategoryConfig {
                probability: 1.0,
                data_types: vec![],
                secondary_categories: vec![
                    "ai".to_string(),
                    "blockchain".to_string(),
                    "software".to_string(),
                ],
                file_extensions: vec!["json".to_string(), "txt".to_string(), "csv".to_string()],
            },
        );
        categories_supported.insert(
            "science".to_string(),
            CategoryConfig {
                probability: 1.0,
                data_types: vec![],
                secondary_categories: vec![
                    "physics".to_string(),
                    "chemistry".to_string(),
                    "biology".to_string(),
                ],
                file_extensions: vec!["json".to_string(), "xml".to_string(), "txt".to_string()],
            },
        );
        categories_supported.insert(
            "business".to_string(),
            CategoryConfig {
                probability: 1.0,
                data_types: vec![],
                secondary_categories: vec![
                    "finance".to_string(),
                    "marketing".to_string(),
                    "strategy".to_string(),
                ],
                file_extensions: vec!["json".to_string(), "csv".to_string(), "xlsx".to_string()],
            },
        );

        log::debug!(
            "🏷️ Added {} example category groups",
            categories_supported.len()
        );

        let example_config = AppConfig {
            target_chain: "solana".to_string(),
            solana_rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            evm_config: None,
            database_path: "./kv_store".to_string(),
            min_daily_transactions: 100,
            max_daily_transactions: 100,
            min_user_key_expiry_seconds: 3600,
            max_user_key_expiry_seconds: 86400,
            user_key_pool_size: 100,
            high_rating_percentage: 0.8,
            cron_schedule_in_minutes: 15,
            min_file_size_bytes: 512,
            max_file_size_bytes: 4096,
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
