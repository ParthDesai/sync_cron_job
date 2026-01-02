use crate::config::{AppConfig, EvmConfig};
use crate::database::{Database, UserKeyRecord};
use crate::errors::{Result, SyncCronError};
use crate::pinata_client::PinataClient;
use crate::solana_client::TransactionResult;
use rand::Rng;
use serde_json::Value;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[derive(Clone)]
pub struct EvmClient {
    config: EvmConfig,
    app_config: AppConfig,
    pinata_client: PinataClient,
    database: Database,
    bridge_script_path: String,
}

impl EvmClient {
    pub fn new(
        config: EvmConfig,
        app_config: AppConfig,
        pinata_client: PinataClient,
        database: Database,
        bridge_script_path: String,
    ) -> Self {
        Self {
            config,
            app_config,
            pinata_client,
            database,
            bridge_script_path,
        }
    }

    async fn run_bridge(&self, command: &str, input: Value) -> Result<Value> {
        let mut child = Command::new("node")
            .arg(&self.bridge_script_path)
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SyncCronError::Network(format!("Failed to spawn node: {}", e)))?;

        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| SyncCronError::Network("Failed to open stdin for node".into()))?;
            let payload = serde_json::to_vec(&input)?;
            stdin
                .write_all(&payload)
                .await
                .map_err(|e| SyncCronError::Network(format!("Failed to write to node stdin: {}", e)))?;
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| SyncCronError::Network(format!("Failed to wait for node: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(SyncCronError::Network(format!(
                "evm_bridge failed (cmd={}): {}",
                command, stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let v: Value = serde_json::from_str(stdout.trim())
            .map_err(|e| SyncCronError::Network(format!("Invalid JSON from evm_bridge: {}", e)))?;
        Ok(v)
    }

    async fn get_balance_wei(&self, address: &str) -> Result<u128> {
        let v = self
            .run_bridge(
                "get_balance",
                serde_json::json!({ "rpcUrl": self.config.rpc_url, "address": address }),
            )
            .await?;
        let wei_str = v["wei"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing wei in get_balance response".into()))?;
        wei_str
            .parse::<u128>()
            .map_err(|e| SyncCronError::Network(format!("Invalid wei returned: {}", e)))
    }

    async fn fund_if_needed(&self, address: &str, min_balance_wei: u128) -> Result<()> {
        let Some(admin_pk) = &self.config.admin_private_key else {
            return Ok(());
        };

        let bal = self.get_balance_wei(address).await?;
        if bal >= min_balance_wei {
            return Ok(());
        }

        let fund_amount = self.config.funding_amount_wei;
        log::info!(
            "💰 Funding EVM address {} with {} wei (balance was {})",
            address,
            fund_amount,
            bal
        );

        let _ = self
            .run_bridge(
                "send_eth",
                serde_json::json!({
                    "rpcUrl": self.config.rpc_url,
                    "fromPrivateKey": admin_pk,
                    "to": address,
                    "wei": fund_amount.to_string()
                }),
            )
            .await?;
        Ok(())
    }

    async fn ensure_agent_ready(&self, agent_pk: &str) -> Result<String> {
        // Derive address
        let v = self
            .run_bridge(
                "derive_address",
                serde_json::json!({ "privateKey": agent_pk }),
            )
            .await?;
        let agent_addr = v["address"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing address in derive_address".into()))?
            .to_string();

        self.fund_if_needed(&agent_addr, self.config.min_agent_balance_wei)
            .await?;

        // Create agent config if needed
        let cfg = self
            .run_bridge(
                "agent_config",
                serde_json::json!({
                    "rpcUrl": self.config.rpc_url,
                    "proxyAddress": self.config.sync_contract_proxy,
                    "address": agent_addr
                }),
            )
            .await?;
        let exists = cfg["exists"].as_bool().unwrap_or(false);
        let is_enabled = cfg["isEnabled"].as_bool().unwrap_or(false);

        if !exists {
            log::info!("🧩 Creating on-chain agent config for {}", agent_addr);
            let _ = self
                .run_bridge(
                    "create_agent",
                    serde_json::json!({
                        "rpcUrl": self.config.rpc_url,
                        "proxyAddress": self.config.sync_contract_proxy,
                        "privateKey": agent_pk
                    }),
                )
                .await?;
        }

        if !is_enabled {
            if let Some(admin_pk) = &self.config.admin_private_key {
                log::info!("✅ Allowing agent {} (admin)", agent_addr);
                let _ = self
                    .run_bridge(
                        "allow_agent",
                        serde_json::json!({
                            "rpcUrl": self.config.rpc_url,
                            "proxyAddress": self.config.sync_contract_proxy,
                            "adminPrivateKey": admin_pk,
                            "agent": agent_addr
                        }),
                    )
                    .await?;
            } else {
                log::warn!(
                    "⚠️ Agent {} is not enabled, but EVM_ADMIN_PRIVATE_KEY is not set so we cannot allow it",
                    agent_addr
                );
            }
        }

        Ok(agent_addr)
    }

    async fn create_random_wallet(&self) -> Result<(String, String)> {
        let v = self
            .run_bridge("create_random_wallet", serde_json::json!({}))
            .await?;
        let address = v["address"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing address in create_random_wallet".into()))?
            .to_string();
        let private_key = v["privateKey"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing privateKey in create_random_wallet".into()))?
            .to_string();
        Ok((address, private_key))
    }

    pub async fn get_or_create_user_key(&self) -> Result<UserKeyRecord> {
        self.database
            .get_or_create_user_key_for_chain(
                crate::database::ChainTarget::Base,
                self.app_config.user_key_pool_size,
                self.app_config.min_user_key_expiry_seconds,
                self.app_config.max_user_key_expiry_seconds,
                Some(self.bridge_script_path.as_str()),
            )
            .await
    }

    pub async fn submit_transaction(&self) -> Result<TransactionResult> {
        // Pick file profile
        let (category, data_type, format, file_extension) = self.app_config.select_file_profile()?;

        let (file_name, data) = crate::solana_client::SolanaClient::generate_random_file_name_and_data_static(
            &category,
            &data_type,
            &format,
            &file_extension,
            self.app_config.min_file_size_bytes,
            self.app_config.max_file_size_bytes,
        )?;

        // Rounded-up KB for on-chain storage.
        let file_size_in_kb: u128 = ((data.len() as u128) + 1023) / 1024;

        let upload_result = self
            .pinata_client
            .upload_bytes(data, &file_name, None)
            .await?;
        let data_link = upload_result.cid.clone();

        // User
        let user_key_record = self.get_or_create_user_key().await?;
        let user_addr = user_key_record.pubkey.clone();

        self.fund_if_needed(&user_addr, self.config.min_user_balance_wei)
            .await?;

        // Submit
        let submit = self
            .run_bridge(
                "submit_data",
                serde_json::json!({
                    "rpcUrl": self.config.rpc_url,
                    "proxyAddress": self.config.sync_contract_proxy,
                    "privateKey": user_key_record.private_key,
                    "dataLink": data_link,
                    "primaryCategory": category,
                    "secondaryCategory": data_type,
                    "domain": self.config.domain,
                    "dataType": data_type,
                    "dataFormat": format,
                    "fileSizeInKB": file_size_in_kb.to_string()
                }),
            )
            .await?;
        let submit_tx = submit["txHash"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing txHash from submit_data".into()))?
            .to_string();

        // Agent
        if self.config.agents.is_empty() {
            return Err(SyncCronError::Config("No EVM agents configured".into()));
        }
        let agent_pk = self.config.agents[rand::thread_rng().gen_range(0..self.config.agents.len())].clone();
        let _agent_addr = self.ensure_agent_ready(&agent_pk).await?;

        // Generate rating & validity mapping (keep Solana-like behavior: low rating => invalid => 0 credits)
        let (rating, passed) = crate::solana_client::SolanaClient::generate_random_rating_static(
            self.app_config.high_rating_percentage,
        );
        let is_valid = passed;
        let has_rating = true;

        let synthetic_data_link = if passed {
            let (synthetic_name, synthetic_data) =
                crate::solana_client::SolanaClient::generate_synthetic_file_name_and_data_static(
                    &file_name,
                    self.app_config.min_file_size_bytes,
                    self.app_config.max_file_size_bytes,
                )?;
            let s = self
                .pinata_client
                .upload_bytes(synthetic_data, &synthetic_name, None)
                .await?;
            Some(s.cid)
        } else {
            None
        };

        let rate = self
            .run_bridge(
                "rate_data",
                serde_json::json!({
                    "rpcUrl": self.config.rpc_url,
                    "proxyAddress": self.config.sync_contract_proxy,
                    "privateKey": agent_pk,
                    "dataLink": data_link,
                    "isSeedDeleted": true,
                    "isValid": is_valid,
                    "hasRating": has_rating,
                    "rating": rating,
                    "hasSyntheticDataLink": synthetic_data_link.is_some(),
                    "syntheticDataLink": synthetic_data_link.unwrap_or_default(),
                    "sendTokensImmediately": false
                }),
            )
            .await?;
        let rate_tx = rate["txHash"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing txHash from rate_data".into()))?
            .to_string();

        // Read accumulated credits from chain
        let credits = self
            .run_bridge(
                "accumulated_credits",
                serde_json::json!({
                    "rpcUrl": self.config.rpc_url,
                    "proxyAddress": self.config.sync_contract_proxy,
                    "address": user_addr
                }),
            )
            .await?;
        let credits_u128 = credits["credits"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing credits from accumulated_credits".into()))?
            .parse::<u128>()
            .map_err(|e| SyncCronError::Network(format!("Invalid credits value: {}", e)))?;

        // Update DB record
        let _ = self
            .database
            .update_user_accumulated_credits(&user_key_record.id, credits_u128)
            .await;

        Ok(TransactionResult {
            user_accumulated_credits: credits_u128,
            signatures: vec![submit_tx, rate_tx],
            slot: None,
            block_hash: None,
            confirmation_status: "confirmed".to_string(),
            ipfs_url: upload_result.gateway_url,
            error: None,
        })
    }

    pub async fn claim_credits(&self, user_key_record: &UserKeyRecord) -> Result<(String, u128)> {
        let user_addr = user_key_record.pubkey.clone();
        self.fund_if_needed(&user_addr, self.config.min_user_balance_wei)
            .await?;

        let claim = self
            .run_bridge(
                "claim_credits",
                serde_json::json!({
                    "rpcUrl": self.config.rpc_url,
                    "proxyAddress": self.config.sync_contract_proxy,
                    "privateKey": user_key_record.private_key
                }),
            )
            .await?;
        let tx_hash = claim["txHash"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing txHash from claim_credits".into()))?
            .to_string();

        let credits = self
            .run_bridge(
                "accumulated_credits",
                serde_json::json!({
                    "rpcUrl": self.config.rpc_url,
                    "proxyAddress": self.config.sync_contract_proxy,
                    "address": user_addr
                }),
            )
            .await?;
        let new_credits = credits["credits"]
            .as_str()
            .ok_or_else(|| SyncCronError::Network("Missing credits from accumulated_credits".into()))?
            .parse::<u128>()
            .map_err(|e| SyncCronError::Network(format!("Invalid credits value: {}", e)))?;

        Ok((tx_hash, new_credits))
    }
}

