use crate::config::SolanaConfig;
use crate::errors::{Result, SyncCronError};
use anchor_client::anchor_lang::{
    AccountDeserialize, AnchorDeserialize, Discriminator,
};
use anchor_client::solana_client::rpc_client::{GetConfirmedSignaturesForAddress2Config, RpcClient};

use anchor_client::solana_client::rpc_response::RpcConfirmedTransactionStatusWithSignature;
use anchor_client::solana_sdk::commitment_config::CommitmentLevel;
use anchor_client::solana_sdk::signature::Signature;
use anchor_client::solana_sdk::{
    commitment_config::CommitmentConfig, pubkey::Pubkey
};
use anchor_spl::token_2022::spl_token_2022;
use serde::{Deserialize, Serialize};
use solana_transaction_status_client_types::{
    EncodedTransaction, UiInstruction, UiMessage, UiTransactionEncoding,
    UiTransactionStatusMeta,
};
use std::str::FromStr;
use sync_contract::types::{Datasubmission, UserConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncContractTxData {
    pub transactions: Vec<SyncContractTransaction>,
    pub before_signature: Option<Signature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SyncContractTransaction {
    FileSubmission {
        cid: String,
        rating: Option<u8>,
        credits_earned: Option<u128>,
        user_wallet: String,
        tx_hash: String,
        block_number: u64,
    },
    CreditClaim {
        user_wallet: String,
        credits_claimed: f64,
        tx_hash: String,
        block_number: u64,
    },
}



pub struct SolanaTxFetcher {
    rpc_client: RpcClient,
    config: SolanaConfig
}

impl SolanaTxFetcher {
    pub fn new(rpc_url: &String, config: SolanaConfig) -> Self {
        let rpc_client = RpcClient::new_with_commitment(rpc_url, CommitmentConfig { commitment: CommitmentLevel::Confirmed });
        Self {
            rpc_client,
            config
        }
    }

    fn decode_details_about_claim_credit_instruction(
        &self,
        token_program_index: usize,
        mint_account_index: usize,
        instruction_index: usize,
        instruction_accounts: Vec<u8>,
        account_keys: Vec<String>,
        tx_meta: Option<UiTransactionStatusMeta>,
    ) -> Result<(anchor_spl::token_interface::Mint, u64)> {
        let token_program_index = instruction_accounts[token_program_index];

        let mint_account_key =
            account_keys[(instruction_accounts[mint_account_index]) as usize].clone();
        let mint_account_data_raw = self
            .rpc_client
            .get_account_data(&Pubkey::from_str(&mint_account_key.as_str())?)?;
        let mint_account =
            anchor_spl::token_interface::Mint::try_deserialize(&mut mint_account_data_raw.as_ref())
                .map_err(anyhow::Error::new)?;

        let inner_instructions = tx_meta
            .ok_or(SyncCronError::Network("tx meta must exists".into()))?
            .inner_instructions
            .ok_or(SyncCronError::Network(
                "tx inner instruction must exists".into(),
            ))?;
        let cpi_instructions = inner_instructions
            .iter()
            .find(|inner_instructions| inner_instructions.index as usize == instruction_index)
            .ok_or(SyncCronError::Network(
                "Democlaimcredit must have cpi instructions".into(),
            ))?;
        let token_program_cpi_amount = cpi_instructions.instructions.iter().find_map(|inner_instruction| {
                                        if let UiInstruction::Compiled(compiled_cpi_instruction) = inner_instruction {
                                            if compiled_cpi_instruction.program_id_index == token_program_index {
                                                let maybe_binary_token_program_instruction_data = bs58::decode(&compiled_cpi_instruction.data).into_vec();
                                                if let Ok(binary_token_program_instruction_data) = maybe_binary_token_program_instruction_data {
                                                    let token_instruction = anchor_spl::token_2022::spl_token_2022::instruction::TokenInstruction::unpack(&binary_token_program_instruction_data).unwrap();
                                                    match token_instruction {
                                                        spl_token_2022::instruction::TokenInstruction::MintTo { amount } => return Some(amount),
                                                        _ => {}
                                                    }
                                                }
                                            }
                                        }
                                        return None;
                                    }).ok_or(SyncCronError::Network("Unable to get mint_to token cpi data".into()))?;

        Ok((mint_account, token_program_cpi_amount))
    }

    async fn fetch_tx_data(&self, program_id: Pubkey, signatures: Vec<RpcConfirmedTransactionStatusWithSignature>) -> Result<SyncContractTxData> {
        const DATA_SUBMISSION_ACCOUNT_INDEX_IN_SUBMIT_DATA: usize = 0;
        const USER_CONFIG_ACCOUNT_INDEX_IN_SUBMIT_DATA: usize = 1;
        const USER_ACCOUNT_IN_SUBMIT_DATA: usize = 2;
        const TOKEN_PROGRAM_INDEX_IN_CLAIM_CREDIT: usize = 6;
        const MINT_ACCOUNT_IN_CLAIM_CREDIT: usize = 2;
        const USER_ACCOUNT_IN_CLAIM_CREDIT: usize = 4;
        const TOKEN_PROGRAM_INDEX_IN_DEMO_CLAIM_CREDIT: usize = 6;
        const MINT_ACCOUNT_IN_DEMO_CLAIM_CREDIT: usize = 2;
        const USER_ACCOUNT_IN_DEMO_CLAIM_CREDIT: usize = 4;

        let mut transactions = Vec::new();
        let mut next_cursor = None;

        // Process each signature to create basic transaction entries
        for (i, sig_info) in signatures.iter().enumerate() {
            // Set next cursor to the last signature for pagination
            next_cursor = Some(Signature::from_str(&sig_info.signature).map_err(anyhow::Error::new)?);

            // Skip errored transactions
            if sig_info.err.is_some() {
                log::warn!("⚠️ Skipping errored transaction: {}", sig_info.signature);
                continue;
            }

            let tx = self.rpc_client.get_transaction(
                &Signature::from_str(&sig_info.signature).unwrap(),
                UiTransactionEncoding::Json,
            )?;

            if let EncodedTransaction::Json(tx_data) = tx.transaction.transaction {
                if let UiMessage::Raw(raw_message) = tx_data.message {
                    for (instruction_index, instruction) in
                        raw_message.instructions.iter().enumerate()
                    {
                        let instruction_program_id = Pubkey::from_str(
                            &raw_message.account_keys[instruction.program_id_index as usize],
                        )?;
                        if instruction_program_id == program_id {
                            let binary_instruction_data =
                                bs58::decode(&instruction.data).into_vec()?;

                            let instruction_discriminator: &[u8] = &binary_instruction_data[0..8];
                            let actual_data: &[u8] = &binary_instruction_data[8..];

                            match instruction_discriminator {
                                sync_contract::instruction::SubmitData::DISCRIMINATOR => {
                                    let submit_data_instruction =
                                        sync_contract::instruction::SubmitData::deserialize(
                                            &mut actual_data.as_ref(),
                                        )?;
                                    let data_submission_account_key = raw_message.account_keys
                                        [(instruction.accounts
                                            [DATA_SUBMISSION_ACCOUNT_INDEX_IN_SUBMIT_DATA])
                                            as usize]
                                        .clone();
                                    let data_submission_data_raw =
                                        self.rpc_client.get_account_data(&Pubkey::from_str(
                                            &data_submission_account_key.as_str(),
                                        )?)?;
                                    let data_submission = Datasubmission::try_deserialize(
                                        &mut data_submission_data_raw.as_ref(),
                                    )
                                    .map_err(anyhow::Error::new)?;

                                    let user_config_account_key = raw_message.account_keys
                                        [(instruction.accounts
                                            [USER_CONFIG_ACCOUNT_INDEX_IN_SUBMIT_DATA])
                                            as usize]
                                        .clone();
                                    let user_config_account_data_raw =
                                        self.rpc_client.get_account_data(&Pubkey::from_str(
                                            &user_config_account_key.as_str(),
                                        )?)?;
                                    let user_config = UserConfig::try_deserialize(
                                        &mut user_config_account_data_raw.as_ref(),
                                    )
                                    .map_err(anyhow::Error::new)?;

                                    let tx = SyncContractTransaction::FileSubmission {
                                        cid: String::from_utf8(data_submission.data_link.to_vec())?
                                            .trim_end_matches('\0')
                                            .to_string(),
                                        rating: None,
                                        credits_earned: data_submission
                                            .agent_response
                                            .and_then(|response| Some(response.calculated_credits)),
                                        user_wallet: raw_message.account_keys[(instruction.accounts
                                            [USER_ACCOUNT_IN_SUBMIT_DATA])
                                            as usize]
                                            .clone(),
                                        tx_hash: sig_info.signature.clone(),
                                        block_number: sig_info.slot,
                                    };

                                    transactions.push(tx);
                                }
                                sync_contract::instruction::ClaimCredits::DISCRIMINATOR => {
                                    let _claim_credit_instruction =
                                        sync_contract::instruction::ClaimCredits::deserialize(
                                            &mut actual_data.as_ref(),
                                        )?;
                                    let (mint_account, amount) = self
                                        .decode_details_about_claim_credit_instruction(
                                            TOKEN_PROGRAM_INDEX_IN_CLAIM_CREDIT,
                                            MINT_ACCOUNT_IN_CLAIM_CREDIT,
                                            instruction_index,
                                            instruction.accounts.clone(),
                                            raw_message.account_keys.clone(),
                                            tx.transaction.meta.clone(),
                                        )?;

                                    let tx = SyncContractTransaction::CreditClaim {
                                        user_wallet: raw_message.account_keys[(instruction.accounts
                                            [USER_ACCOUNT_IN_CLAIM_CREDIT])
                                            as usize]
                                            .clone(),
                                        credits_claimed: spl_token_2022::amount_to_ui_amount(amount, mint_account.decimals),
                                        tx_hash: sig_info.signature.clone(),
                                        block_number: sig_info.slot,
                                    };

                                    transactions.push(tx);
                                }
                                sync_contract::instruction::DemoClaimCredits::DISCRIMINATOR => {
                                    let demo_claim_credit_instruction =
                                        sync_contract::instruction::DemoClaimCredits::deserialize(
                                            &mut actual_data.as_ref(),
                                        )?;
                                    let (mint_account, amount) = self
                                        .decode_details_about_claim_credit_instruction(
                                            TOKEN_PROGRAM_INDEX_IN_DEMO_CLAIM_CREDIT,
                                            MINT_ACCOUNT_IN_DEMO_CLAIM_CREDIT,
                                            instruction_index,
                                            instruction.accounts.clone(),
                                            raw_message.account_keys.clone(),
                                            tx.transaction.meta.clone(),
                                        )?;

                                    let tx = SyncContractTransaction::CreditClaim {
                                        user_wallet: raw_message.account_keys[(instruction.accounts
                                            [USER_ACCOUNT_IN_DEMO_CLAIM_CREDIT])
                                            as usize]
                                            .clone(),
                                        credits_claimed: spl_token_2022::amount_to_ui_amount(amount, mint_account.decimals),
                                        tx_hash: sig_info.signature.clone(),
                                        block_number: sig_info.slot,
                                    };

                                    transactions.push(tx);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            };
        }

        log::info!(
            "✅ Processed {} sync_contract transactions",
            transactions.len()
        );

        Ok(SyncContractTxData {
            transactions,
            before_signature: next_cursor,
        })
    }

    /// Fetch transaction data from blockchain for sync_contract
    pub async fn fetch_sync_contract_transactions(
        &self,
        before_signature: Option<Signature>,
        until_signature: Option<Signature>,
        limit: Option<usize>
    ) -> Result<SyncContractTxData> {
        log::info!("🔍 Fetching sync_contract transactions from blockchain...");

        let program_id = Pubkey::from_str(&self.config.program_id)
            .map_err(|_| SyncCronError::Config("Invalid program_id in config".to_string()))?;

        // Fetch signatures for the program address (simplified approach)
        let signatures = self.rpc_client.get_signatures_for_address_with_config(&program_id, GetConfirmedSignaturesForAddress2Config {
            before: before_signature,
            until: until_signature,
            limit: limit,
            commitment: Some(CommitmentConfig { commitment: CommitmentLevel::Finalized }),
        })?;

        log::info!("📝 Found {} signatures for sync_contract", signatures.len());

        self.fetch_tx_data(program_id, signatures).await
    }
}