use crate::config::{AppConfig, SolanaConfig};
use crate::database::Database;
use crate::errors::{Result, SyncCronError};
use crate::pinata_client::PinataClient;
use anchor_client::anchor_lang::prelude::System;
use anchor_client::anchor_lang::Id;
use anchor_client::solana_client::rpc_client::RpcClient;
use anchor_client::solana_sdk::{
    commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Keypair, signer::Signer,
    transaction::Transaction,
};
use anchor_client::solana_sdk::{keccak, system_instruction};
use anchor_client::{Client, Cluster};
use anchor_spl::associated_token::spl_associated_token_account;
use anchor_spl::token_2022::spl_token_2022;
use rand::Rng;
use serde::{Deserialize, Serialize};
use shellexpand;
use std::str::FromStr;
use sync_contract::types::UserConfig;

pub struct SolanaClient {
    rpc_client: RpcClient,
    config: SolanaConfig,
    pinata_client: PinataClient,
    agents: Vec<Keypair>, // Loaded agent keypairs
    database: Database,
    app_config: AppConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionResult {
    pub signatures: Vec<String>,
    pub user_accumulated_credits: u128,
    pub ipfs_url: String,
    pub slot: Option<u64>,
    pub block_hash: Option<String>,
    pub confirmation_status: String,
    pub error: Option<String>,
}

impl SolanaClient {
    pub fn new(
        rpc_url: &str,
        config: SolanaConfig,
        pinata_client: PinataClient,
        agents: Vec<Keypair>,
        database: Database,
        app_config: AppConfig,
    ) -> Self {
        let rpc_client =
            RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());

        log::info!(
            "🌐 Initialized SolanaClient with {} agent keypairs",
            agents.len()
        );

        Self {
            rpc_client,
            config,
            pinata_client,
            agents,
            database,
            app_config,
        }
    }

    fn generate_random_file_name_and_data(
        &self,
        primary_category: &str,
        secondary_category: &str,
        file_extension: &str,
    ) -> Result<(String, Vec<u8>)> {
        // Generate filename from categories
        let file_name = format!(
            "{}_{}.{}",
            primary_category, secondary_category, file_extension
        );

        // Generate random file size between min and max
        let mut rng = rand::thread_rng();
        let file_size = rng
            .gen_range(self.app_config.min_file_size_bytes..=self.app_config.max_file_size_bytes)
            as usize;

        let mut data = vec![0; file_size];
        // Fill data vector with random data
        rng.fill(&mut data[..]);

        log::info!("📄 Generated file: {} ({} bytes)", file_name, file_size);

        Ok((file_name, data))
    }

    fn transfer_funds(&self, from: &Keypair, to: &Pubkey, amount: u64) -> Result<()> {
        // Create transfer instruction using system program
        let transfer_instruction = system_instruction::transfer(&from.pubkey(), to, amount);

        // Get recent blockhash for the transaction
        let recent_blockhash = self.rpc_client.get_latest_blockhash()?;

        // Create and sign transaction
        let transaction = Transaction::new_signed_with_payer(
            &[transfer_instruction],
            Some(&from.pubkey()), // Payer
            &[from],              // Signers
            recent_blockhash,
        );

        // Send and confirm transaction
        self.rpc_client.send_and_confirm_transaction(&transaction)?;

        log::info!(
            "💸 Transferred {} lamports from {} to {}",
            amount,
            from.pubkey(),
            to
        );

        Ok(())
    }

    async fn fund_user_if_needed(
        &self,
        master_keypair: &Keypair,
        user_pubkey: &Pubkey,
    ) -> Result<()> {
        const MIN_BALANCE_LAMPORTS: u64 = 10_000_000; // 0.01 SOL in lamports

        let balance = self.get_balance(user_pubkey).await?;

        if balance < MIN_BALANCE_LAMPORTS {
            let funding_amount = MIN_BALANCE_LAMPORTS * 2; // Fund with 0.02 SOL to avoid frequent funding
            self.transfer_funds(master_keypair, user_pubkey, funding_amount)?;
            log::info!(
                "💰 Funded user {} with {} lamports (balance was {})",
                user_pubkey,
                funding_amount,
                balance
            );
        } else {
            log::info!(
                "✅ User {} balance {} lamports is sufficient (>= {})",
                user_pubkey,
                balance,
                MIN_BALANCE_LAMPORTS
            );
        }

        Ok(())
    }

    async fn fund_agent_if_needed(
        &self,
        master_keypair: &Keypair,
        agent_pubkey: &Pubkey,
    ) -> Result<()> {
        const MIN_BALANCE_LAMPORTS: u64 = 10_000_000; // 0.01 SOL in lamports
        const AGENT_FUNDING_AMOUNT: u64 = 100_000_000; // 0.1 SOL in lamports

        let balance = self.get_balance(agent_pubkey).await?;

        if balance < MIN_BALANCE_LAMPORTS {
            self.transfer_funds(master_keypair, agent_pubkey, AGENT_FUNDING_AMOUNT)?;
            log::info!(
                "💰 Funded agent {} with {} lamports (balance was {})",
                agent_pubkey,
                AGENT_FUNDING_AMOUNT,
                balance
            );
        } else {
            log::info!(
                "✅ Agent {} balance {} lamports is sufficient (>= {})",
                agent_pubkey,
                balance,
                MIN_BALANCE_LAMPORTS
            );
        }

        Ok(())
    }

    pub async fn submit_transaction(
        &self,
        _solana_config: &SolanaConfig,
    ) -> Result<TransactionResult> {
        // Load the master keypair for signing from private key
        let master_keypair = self.load_keypair(&self.config.keypair_file)?;

        // Check master keypair balance and warn if low
        let master_balance = self.get_balance(&master_keypair.pubkey()).await?;
        const MIN_MASTER_BALANCE_LAMPORTS: u64 = 500_000_000; // 0.5 SOL in lamports

        if master_balance < MIN_MASTER_BALANCE_LAMPORTS {
            log::warn!(
                "⚠️  WARNING: Master keypair balance is LOW! Current balance: {} SOL ({} lamports). Consider funding the master keypair to avoid transaction failures.",
                master_balance as f64 / 1_000_000_000.0,
                master_balance
            );
        } else {
            log::info!(
                "💰 Master keypair balance: {} SOL ({} lamports)",
                master_balance as f64 / 1_000_000_000.0,
                master_balance
            );
        }

        let cluster = Cluster::from_str(&self.rpc_client.url())?;
        let client =
            Client::new_with_options(cluster, &master_keypair, CommitmentConfig::confirmed());

        let program_id = Pubkey::from_str(&self.config.program_id).unwrap();
        let program = client.program(program_id).unwrap();

        // Get random categories and file extension for the transaction first
        let (primary_category, secondary_category, file_extension) =
            self.app_config.get_random_categories()?;
        log::info!(
            "🎯 Selected categories - Primary: '{}', Secondary: '{}', Extension: '{}'",
            primary_category,
            secondary_category,
            file_extension
        );

        let (file_name, data) = self.generate_random_file_name_and_data(
            &primary_category,
            &secondary_category,
            &file_extension,
        )?;
        let upload_result = self
            .pinata_client
            .upload_bytes(data, &file_name, None)
            .await?;
        log::info!("📁 Uploaded file to Pinata: {}", upload_result.gateway_url);

        let (data_submission, _) = Pubkey::find_program_address(
            &[
                b"sync_program".as_ref(),
                b"data_submission".as_ref(),
                keccak::hash(upload_result.cid.as_bytes()).as_ref(),
            ],
            &program_id,
        );

        // Get user keypair from pool instead of creating new one
        let user_key_record = self
            .database
            .get_or_create_user_key(
                self.app_config.user_key_pool_size,
                self.app_config.min_user_key_expiry_seconds,
                self.app_config.max_user_key_expiry_seconds,
            )
            .await?;

        // Load the keypair from the stored private key
        let user = Self::load_keypair_from_private_key_static(&user_key_record.private_key)?;

        // Fund user only if balance is below 0.01 SOL
        self.fund_user_if_needed(&master_keypair, &user.pubkey())
            .await?;

        let (user_config, _) = Pubkey::find_program_address(
            &[
                b"sync_program".as_ref(),
                b"user_config".as_ref(),
                user.pubkey().as_ref(),
            ],
            &program.id(),
        );

        // Submit data transaction - waits for confirmation
        let submitdata_signature = program
            .request()
            .accounts(sync_contract::accounts::SubmitData {
                data_submission,
                signer: user.pubkey(),
                system_program: System::id(),
                user_config,
            })
            .args(sync_contract::instruction::SubmitData {
                data_link: upload_result.cid.clone(),
                primary_category,
                secondary_category,
            })
            .payer(&user)
            .send()
            .await?;

        log::info!(
            "🚀 User {} submitted data to blockchain, tx hash: {}",
            user.pubkey(),
            submitdata_signature.to_string()
        );

        // Choose random agent from available agents and send rate data for the same file
        let agent = self.get_random_agent().unwrap();
        log::info!("🎲 Chose agent: {}", agent.pubkey());

        // Fund agent only if balance is below 0.01 SOL
        self.fund_agent_if_needed(&master_keypair, &agent.pubkey())
            .await?;

        let (agent_config, _) = Pubkey::find_program_address(
            &[
                b"sync_program".as_ref(),
                b"agent_config".as_ref(),
                agent.pubkey().as_ref(),
            ],
            &program.id(),
        );

        let random_rating = self.generate_random_rating();
        log::info!(
            "💎 Generated random rating: {} by agent: {}",
            random_rating,
            agent.pubkey()
        );

        // Rate data transaction - waits for confirmation
        let rate_data_signature = program
            .request()
            .accounts(sync_contract::accounts::RateData {
                data_submission,
                agent_config,
                signer: agent.pubkey(),
                user_config,
            })
            .args(sync_contract::instruction::RateData {
                data_link: upload_result.cid.clone(),
                passed: true,
                rating: random_rating,
            })
            .payer(&agent)
            .send()
            .await?;

        log::info!(
            "✅ Agent rated data, tx hash: {}",
            rate_data_signature.to_string()
        );

        let user_config_content: UserConfig = program.account(user_config.clone()).await?;

        let user_accumulated_credits = user_config_content.accumulated_credits;

        // Update the user's accumulated credits in the database
        if let Err(e) = self
            .database
            .update_user_accumulated_credits(&user_key_record.id, user_accumulated_credits)
            .await
        {
            log::error!("❌ Failed to update accumulated credits in database: {}", e);
        }

        // Both transactions are confirmed when we reach this point
        Ok(TransactionResult {
            user_accumulated_credits,
            signatures: vec![
                submitdata_signature.to_string(),
                rate_data_signature.to_string(),
            ],
            slot: None,
            block_hash: None,
            confirmation_status: "confirmed".to_string(),
            ipfs_url: upload_result.gateway_url,
            error: None,
        })
    }

    // Load keypair from base58 encoded private key
    pub fn load_keypair_from_private_key(&self, private_key: &str) -> Result<Keypair> {
        Self::load_keypair_from_private_key_static(private_key)
    }

    // Static method to load keypair from base58 encoded private key
    pub fn load_keypair_from_private_key_static(private_key: &str) -> Result<Keypair> {
        // Decode base58 private key
        let private_key_bytes = bs58::decode(private_key)
            .into_vec()
            .map_err(|e| SyncCronError::Config(format!("Invalid private key format: {}", e)))?;

        // Create keypair from private key bytes
        Ok(Keypair::try_from(&private_key_bytes[..])
            .map_err(|e| SyncCronError::Config(format!("Invalid keypair format: {}", e)))?)
    }

    // Deprecated: kept for backward compatibility
    pub fn load_keypair(&self, path: &str) -> Result<Keypair> {
        Self::load_keypair_static(path)
    }

    // Deprecated: kept for backward compatibility
    pub fn load_keypair_static(path: &str) -> Result<Keypair> {
        let expanded_path = shellexpand::tilde(path);
        log::info!("🔑 Loading keypair from: {}", expanded_path);
        let keypair_bytes = std::fs::read(&*expanded_path)?;
        let keypair: Vec<u8> = serde_json::from_slice(&keypair_bytes)?;

        // Use try_from instead of deprecated from_bytes method
        Ok(Keypair::try_from(&keypair[..])
            .map_err(|e| SyncCronError::Config(format!("Invalid keypair format: {}", e)))?)
    }

    pub async fn get_balance(&self, pubkey: &Pubkey) -> Result<u64> {
        Ok(self.rpc_client.get_balance(pubkey)?)
    }

    pub async fn get_current_slot(&self) -> Result<u64> {
        Ok(self.rpc_client.get_slot()?)
    }

    // Agent management methods
    pub fn get_agents(&self) -> &Vec<Keypair> {
        &self.agents
    }

    pub fn get_agent(&self, index: usize) -> Option<&Keypair> {
        self.agents.get(index)
    }

    pub fn get_random_agent(&self) -> Option<&Keypair> {
        if self.agents.is_empty() {
            return None;
        }

        use rand::Rng;
        let mut rng = rand::thread_rng();
        let index = rng.gen_range(0..self.agents.len());
        self.agents.get(index)
    }

    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    pub fn get_agent_pubkeys(&self) -> Vec<Pubkey> {
        self.agents.iter().map(|agent| agent.pubkey()).collect()
    }

    // Generate a random rating based on the configured percentage
    fn generate_random_rating(&self) -> u8 {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        // Decide whether to give a high rating (>80) or low rating (<=80)
        let random_percentage: f64 = rng.gen();

        if random_percentage < self.app_config.high_rating_percentage {
            // Give a high rating (81-100)
            rng.gen_range(81..=100)
        } else {
            // Give a low rating (0-79)
            rng.gen_range(0..=79)
        }
    }

    /// Claim accumulated credits for a user
    pub async fn claim_credits(
        &self,
        user_key_record: &crate::database::UserKeyRecord,
    ) -> Result<(String, u128)> {
        // Load the master keypair for paying transaction fees
        let master_keypair = self.load_keypair(&self.config.keypair_file)?;

        // Load the user keypair from the stored private key
        let user = Self::load_keypair_from_private_key_static(&user_key_record.private_key)?;

        // Fund user only if balance is below 0.01 SOL
        self.fund_user_if_needed(&master_keypair, &user.pubkey())
            .await?;

        let cluster = Cluster::from_str(&self.rpc_client.url())?;
        let client =
            Client::new_with_options(cluster, &master_keypair, CommitmentConfig::confirmed());

        let program_id = Pubkey::from_str(&self.config.program_id).unwrap();
        let program = client.program(program_id).unwrap();

        let (user_config, _) = Pubkey::find_program_address(
            &[
                b"sync_program".as_ref(),
                b"user_config".as_ref(),
                user.pubkey().as_ref(),
            ],
            &program.id(),
        );

        let user_config_content: UserConfig = program.account(user_config.clone()).await?;

        log::info!(
            "💳 Claiming {} credits for user as per our records: {}, as per on chain: {}",
            user_key_record.accumulated_credits,
            user.pubkey(),
            user_config_content.accumulated_credits
        );

        // Get the program state PDA
        let (program_state, _) = Pubkey::find_program_address(
            &[b"sync_program".as_ref(), b"global_state".as_ref()],
            &program.id(),
        );

        // Get the token mint address
        let token_mint = Pubkey::from_str(&self.config.token_mint_address).unwrap();

        // Create a placeholder associated token account address
        // This would normally be calculated using the SPL associated token account program
        let user_token_account =
            spl_associated_token_account::get_associated_token_address_with_program_id(
                &user.pubkey(),
                &token_mint,
                &spl_token_2022::id(),
            );

        // Call ClaimCredits instruction - waits for confirmation
        let claim_credits_signature = program
            .request()
            .accounts(sync_contract::accounts::ClaimCredits {
                program_state,
                mint: token_mint,
                token_account: user_token_account,
                user_config,
                signer: user.pubkey(),
                token_program: spl_token_2022::id(),
                associated_token_program: spl_associated_token_account::id(),
                system_program: System::id(),
            })
            .args(sync_contract::instruction::ClaimCredits {})
            .payer(&user)
            .send()
            .await?;

        let user_config_content: UserConfig = program.account(user_config.clone()).await?;

        log::info!(
            "✅ Successfully claimed credits for user {}, tx hash: {}, new accumulated credits: {}",
            user.pubkey(),
            claim_credits_signature.to_string(),
            user_config_content.accumulated_credits
        );

        Ok((
            claim_credits_signature.to_string(),
            user_config_content.accumulated_credits,
        ))
    }

    // Utility method to load multiple keypairs from private keys
    pub fn load_agents_from_private_keys(private_keys: &[String]) -> Vec<Keypair> {
        let mut agents = Vec::new();
        for (index, private_key) in private_keys.iter().enumerate() {
            match Self::load_keypair_from_private_key_static(private_key) {
                Ok(keypair) => {
                    log::info!(
                        "🔑 Loaded agent keypair #{}: {}",
                        index + 1,
                        keypair.pubkey()
                    );
                    agents.push(keypair);
                }
                Err(e) => {
                    log::warn!("❌ Failed to load agent keypair #{}: {}", index + 1, e);
                    // Continue loading other agents even if one fails
                }
            }
        }
        agents
    }

    // Deprecated: kept for backward compatibility
    pub fn load_agents_from_paths(paths: &[String]) -> Vec<Keypair> {
        let mut agents = Vec::new();
        for path in paths {
            match Self::load_keypair_static(path) {
                Ok(keypair) => {
                    log::info!("🔑 Loaded agent keypair from: {}", path);
                    agents.push(keypair);
                }
                Err(e) => {
                    log::warn!("❌ Failed to load agent keypair from {}: {}", path, e);
                    // Continue loading other agents even if one fails
                }
            }
        }
        agents
    }
}
