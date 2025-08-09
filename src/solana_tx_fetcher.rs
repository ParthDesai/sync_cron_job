use crate::config::SolanaConfig;
use crate::errors::{Result, SyncCronError};
use anchor_client::anchor_lang::{AccountDeserialize, AnchorDeserialize, Discriminator};
use anchor_client::solana_client::nonblocking::rpc_client::RpcClient;

use anchor_client::solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use anchor_client::solana_client::rpc_response::RpcConfirmedTransactionStatusWithSignature;
use anchor_client::solana_sdk::commitment_config::CommitmentLevel;
use anchor_client::solana_sdk::signature::Signature;
use anchor_client::solana_sdk::{commitment_config::CommitmentConfig, pubkey::Pubkey};
use anchor_spl::token_2022::spl_token_2022;
use crossterm::cursor::MoveToColumn;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::{terminal, QueueableCommand};
use serde::{Deserialize, Serialize};
use solana_transaction_status_client_types::{
    EncodedTransaction, UiInstruction, UiMessage, UiTransactionEncoding, UiTransactionStatusMeta,
};
use std::io::{self, stdout, Write};
use std::str::FromStr;
use sync_contract::types::{Datasubmission, UserConfig};
use tokio::io::AsyncWriteExt;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::{join, select};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncContractTxData {
    pub transactions: Vec<SyncContractInstruction>,
    pub before_signature: Option<String>,
    pub error: Option<String>,
}

pub struct PrintTask {
    pub text_color: Color,
    pub should_add_new_line: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SyncContractInstruction {
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
    UnknownInstruction {
        tx_hash: String,
        block_number: u64,
    },
}

impl SyncContractInstruction {
    pub fn get_tx_hash(&self) -> String {
        match &self {
            SyncContractInstruction::FileSubmission { tx_hash, .. } => tx_hash.clone(),
            SyncContractInstruction::CreditClaim { tx_hash, .. } => tx_hash.clone(),
            SyncContractInstruction::UnknownInstruction { tx_hash, .. } => tx_hash.clone(),
        }
    }

    pub fn is_unknown(&self) -> bool {
        match &self {
            SyncContractInstruction::FileSubmission { .. } => false,
            SyncContractInstruction::CreditClaim { .. } => false,
            SyncContractInstruction::UnknownInstruction { .. } => true,
        }
    }
}

pub struct SolanaTxFetcher {
    rpc_url: String,
    config: SolanaConfig,
}

impl SolanaTxFetcher {
    pub fn new(rpc_url: String, config: SolanaConfig) -> Self {
        Self { rpc_url, config }
    }

    async fn decode_details_about_claim_credit_instruction(
        rpc_client: &RpcClient,
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
        let mint_account_data_raw = rpc_client
            .get_account_data(&Pubkey::from_str(&mint_account_key.as_str())?)
            .await?;
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

    async fn fetch_instruction_data(
        rpc_client: &RpcClient,
        instruction_sender: mpsc::Sender<Option<SyncContractInstruction>>,
        program_id: Pubkey,
        signatures: Vec<RpcConfirmedTransactionStatusWithSignature>,
        print_task_sender: mpsc::Sender<PrintTask>,
    ) -> Result<(Option<Signature>, bool)> {
        const DATA_SUBMISSION_ACCOUNT_INDEX_IN_SUBMIT_DATA: usize = 0;
        const USER_CONFIG_ACCOUNT_INDEX_IN_SUBMIT_DATA: usize = 1;
        const USER_ACCOUNT_IN_SUBMIT_DATA: usize = 2;
        const USER_ACCOUNT_IN_OLD_SUBMIT_DATA: usize = 1;
        const TOKEN_PROGRAM_INDEX_IN_CLAIM_CREDIT: usize = 6;
        const MINT_ACCOUNT_IN_CLAIM_CREDIT: usize = 2;
        const USER_ACCOUNT_IN_CLAIM_CREDIT: usize = 4;
        const TOKEN_PROGRAM_INDEX_IN_DEMO_CLAIM_CREDIT: usize = 6;
        const MINT_ACCOUNT_IN_DEMO_CLAIM_CREDIT: usize = 2;
        const USER_ACCOUNT_IN_DEMO_CLAIM_CREDIT: usize = 4;

        let mut last_ingested_signature: Option<String> = None;

        // Process each signature to create basic transaction entries
        for (i, sig_info) in signatures.iter().enumerate() {
            // Skip errored transactions
            if sig_info.err.is_some() {
                log::warn!("⚠️ Skipping errored transaction: {}", sig_info.signature);
                continue;
            }

            print_task_sender
                .send(PrintTask {
                    text_color: Color::DarkBlue,
                    should_add_new_line: false,
                    text: format!(
                        "🔍 Inspecting transaction with hash: {:?}",
                        sig_info.signature
                    ),
                })
                .await
                .map_err(|send_error| SyncCronError::Network(send_error.to_string()))?;

            let tx = rpc_client
                .get_transaction(
                    &Signature::from_str(&sig_info.signature).unwrap(),
                    UiTransactionEncoding::Json,
                )
                .await?;

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

                            let sync_contract_instruction: SyncContractInstruction;

                            match instruction_discriminator {
                                sync_contract::instruction::SubmitData::DISCRIMINATOR => {
                                    let is_old_submit_data = instruction.accounts.len() == 3;
                                    let submit_data_instruction =
                                        sync_contract::instruction::SubmitData::deserialize(
                                            &mut actual_data.as_ref(),
                                        )?;
                                    let data_submission_account_key = raw_message.account_keys
                                        [(instruction.accounts
                                            [DATA_SUBMISSION_ACCOUNT_INDEX_IN_SUBMIT_DATA])
                                            as usize]
                                        .clone();
                                    let data_submission_data_raw = rpc_client
                                        .get_account_data(&Pubkey::from_str(
                                            &data_submission_account_key.as_str(),
                                        )?)
                                        .await?;
                                    let data_submission = Datasubmission::try_deserialize(
                                        &mut data_submission_data_raw.as_ref(),
                                    )
                                    .map_err(anyhow::Error::new)?;

                                    let user_account_in_submit_data = if is_old_submit_data {
                                        USER_ACCOUNT_IN_OLD_SUBMIT_DATA
                                    } else {
                                        USER_ACCOUNT_IN_SUBMIT_DATA
                                    };

                                    sync_contract_instruction =
                                        SyncContractInstruction::FileSubmission {
                                            cid: String::from_utf8(
                                                data_submission.data_link.to_vec(),
                                            )?
                                            .trim_end_matches('\0')
                                            .to_string(),
                                            rating: data_submission
                                                .agent_response
                                                .as_ref()
                                                .and_then(|response| Some(response.rating)),
                                            credits_earned: data_submission
                                                .agent_response
                                                .as_ref()
                                                .and_then(|response| {
                                                    Some(response.calculated_credits)
                                                }),
                                            user_wallet: raw_message.account_keys[(instruction
                                                .accounts[user_account_in_submit_data])
                                                as usize]
                                                .clone(),
                                            tx_hash: sig_info.signature.clone(),
                                            block_number: sig_info.slot,
                                        };
                                }
                                sync_contract::instruction::ClaimCredits::DISCRIMINATOR => {
                                    let _claim_credit_instruction =
                                        sync_contract::instruction::ClaimCredits::deserialize(
                                            &mut actual_data.as_ref(),
                                        )?;
                                    let (mint_account, amount) =
                                        Self::decode_details_about_claim_credit_instruction(
                                            rpc_client,
                                            TOKEN_PROGRAM_INDEX_IN_CLAIM_CREDIT,
                                            MINT_ACCOUNT_IN_CLAIM_CREDIT,
                                            instruction_index,
                                            instruction.accounts.clone(),
                                            raw_message.account_keys.clone(),
                                            tx.transaction.meta.clone(),
                                        )
                                        .await?;

                                    sync_contract_instruction =
                                        SyncContractInstruction::CreditClaim {
                                            user_wallet: raw_message.account_keys[(instruction
                                                .accounts[USER_ACCOUNT_IN_CLAIM_CREDIT])
                                                as usize]
                                                .clone(),
                                            credits_claimed: spl_token_2022::amount_to_ui_amount(
                                                amount,
                                                mint_account.decimals,
                                            ),
                                            tx_hash: sig_info.signature.clone(),
                                            block_number: sig_info.slot,
                                        };
                                }
                                sync_contract::instruction::DemoClaimCredits::DISCRIMINATOR => {
                                    let demo_claim_credit_instruction =
                                        sync_contract::instruction::DemoClaimCredits::deserialize(
                                            &mut actual_data.as_ref(),
                                        )?;
                                    let (mint_account, amount) =
                                        Self::decode_details_about_claim_credit_instruction(
                                            rpc_client,
                                            TOKEN_PROGRAM_INDEX_IN_DEMO_CLAIM_CREDIT,
                                            MINT_ACCOUNT_IN_DEMO_CLAIM_CREDIT,
                                            instruction_index,
                                            instruction.accounts.clone(),
                                            raw_message.account_keys.clone(),
                                            tx.transaction.meta.clone(),
                                        )
                                        .await?;

                                    sync_contract_instruction =
                                        SyncContractInstruction::CreditClaim {
                                            user_wallet: raw_message.account_keys[(instruction
                                                .accounts[USER_ACCOUNT_IN_DEMO_CLAIM_CREDIT])
                                                as usize]
                                                .clone(),
                                            credits_claimed: spl_token_2022::amount_to_ui_amount(
                                                amount,
                                                mint_account.decimals,
                                            ),
                                            tx_hash: sig_info.signature.clone(),
                                            block_number: sig_info.slot,
                                        };
                                }
                                _ => {
                                    sync_contract_instruction =
                                        SyncContractInstruction::UnknownInstruction {
                                            tx_hash: sig_info.signature.clone(),
                                            block_number: sig_info.slot,
                                        };
                                }
                            }

                            if !sync_contract_instruction.is_unknown()
                                && !instruction_sender.is_closed()
                            {
                                instruction_sender
                                    .send(Some(sync_contract_instruction.clone()))
                                    .await
                                    .map_err(anyhow::Error::new)?;
                            }
                        }
                    }
                }
            };
            last_ingested_signature = Some(sig_info.signature.clone());
            if instruction_sender.is_closed() {
                break;
            }
        }

        if let Some(signature) = last_ingested_signature {
            Ok((
                Some(
                    Signature::from_str(&signature)
                        .expect("Signature from tx should always be valid"),
                ),
                instruction_sender.is_closed(),
            ))
        } else {
            Ok((None, instruction_sender.is_closed()))
        }
    }

    async fn spawn_tx_data_task(
        program_id: Pubkey,
        rpc_url: String,
        mut before_signature: Option<Signature>,
        until_signature: Option<Signature>,
        instruction_data_sender: mpsc::Sender<Option<SyncContractInstruction>>,
        print_task_sender: mpsc::Sender<PrintTask>,
    ) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            let rpc_client = RpcClient::new_with_commitment(
                rpc_url,
                CommitmentConfig {
                    commitment: CommitmentLevel::Confirmed,
                },
            );
            loop {
                // Fetch signatures for the program address (simplified approach)
                let signature_results = rpc_client
                    .get_signatures_for_address_with_config(
                        &program_id,
                        GetConfirmedSignaturesForAddress2Config {
                            before: before_signature,
                            until: until_signature,
                            limit: None,
                            commitment: Some(CommitmentConfig {
                                commitment: CommitmentLevel::Finalized,
                            }),
                        },
                    )
                    .await;

                if let Ok(signatures) = signature_results {
                    let result = Self::fetch_instruction_data(
                        &rpc_client,
                        instruction_data_sender.clone(),
                        program_id,
                        signatures,
                        print_task_sender.clone(),
                    )
                    .await;
                    if let Ok((maybe_signature, is_channel_closed)) = result {
                        if is_channel_closed {
                            let _ = instruction_data_sender.send(None).await;
                            break;
                        }

                        if let Some(signature) = maybe_signature {
                            before_signature = Some(signature);
                        } else {
                            // We have ingested everything possible in the range
                            let _ = instruction_data_sender.send(None).await;
                            break;
                        }
                    } else {
                        let err = result
                            .err()
                            .expect("Already checked success in if condition; qed");
                        print_task_sender
                            .send(PrintTask {
                                text_color: Color::Red,
                                should_add_new_line: true,
                                text: format!("Error while fetching tx data: {:?}", err),
                            })
                            .await
                            .map_err(|send_error| SyncCronError::Network(send_error.to_string()))?;
                        break;
                    }
                } else {
                    let err = signature_results
                        .expect_err("Already checked for success in if block; qed");
                    print_task_sender
                        .send(PrintTask {
                            text_color: Color::Red,
                            should_add_new_line: true,
                            text: format!("Error while fetching signatures: {:?}", err),
                        })
                        .await
                        .map_err(|send_error| SyncCronError::Network(send_error.to_string()))?;
                    break;
                }
            }
            Ok(())
        })
    }

    async fn spawn_printer_task(
        mut print_data_receiver: mpsc::Receiver<PrintTask>,
    ) -> JoinHandle<io::Result<()>> {
        tokio::spawn(async move {
            let mut stdout = stdout();
            loop {
                match print_data_receiver.recv().await {
                    Some(print_task) => {
                        let mut cmd = stdout.queue(ResetColor)?;
                        if print_task.should_add_new_line {
                            cmd = cmd.queue(Print("\n"))?;
                        } else {
                            cmd = cmd
                                .queue(MoveToColumn(0))?
                                .queue(terminal::Clear(terminal::ClearType::CurrentLine))?
                        }
                        cmd.queue(SetForegroundColor(print_task.text_color))?
                            .queue(Print(print_task.text))?
                            .queue(ResetColor)?;

                        stdout.flush()?;
                    }
                    None => {
                        break;
                    }
                }
            }
            Ok(())
        })
    }

    /// Fetch transaction data from blockchain for sync_contract
    pub async fn fetch_sync_contract_transactions(
        &self,
        before_signature: Option<Signature>,
        until_signature: Option<Signature>,
        mut streaming_file: tokio::fs::File,
    ) -> Result<()> {
        log::info!("🔍 Fetching sync_contract transactions from blockchain...");

        let program_id = Pubkey::from_str(&self.config.program_id)
            .map_err(|_| SyncCronError::Config("Invalid program_id in config".to_string()))?;

        let rpc_url = self.rpc_url.clone();

        let (instruction_data_sender, mut instruction_data_receiver) = mpsc::channel(32);
        let (print_task_sender, print_task_receiver) = mpsc::channel(1);

        let printer_task = Self::spawn_printer_task(print_task_receiver).await;

        let tx_data_fetch_task = Self::spawn_tx_data_task(
            program_id,
            rpc_url,
            before_signature,
            until_signature,
            instruction_data_sender,
            print_task_sender.clone(),
        )
        .await;

        let mut sigint = signal(SignalKind::interrupt())?;
        let mut sigterm = signal(SignalKind::terminate())?;

        let mut break_loop = false;

        // Write opening paranthesis
        streaming_file.write(b"[").await?;

        // To not put semicolon at end
        let mut is_this_first_element = true;

        loop {
            if break_loop {
                break;
            }
            select! {
                    _ = sigint.recv()  => {
                        print_task_sender.send(PrintTask { text_color: Color::Red, should_add_new_line: true, text: "Received SIGINT, Shutting down after everything pending is written\n".into() }).await.map_err(|send_error| SyncCronError::Network(send_error.to_string()))?;
                        instruction_data_receiver.close();
                    },
                    _ = sigterm.recv() => {
                        print_task_sender.send(PrintTask { text_color: Color::Red, should_add_new_line: true, text: "Received SIGTERM, Shutting down after everything pending is written\n".into() }).await.map_err(|send_error| SyncCronError::Network(send_error.to_string()))?;
                        instruction_data_receiver.close();
                    },
                    maybe_instruction_data = instruction_data_receiver.recv() => {
                        if let Some(inner_tx_data) = maybe_instruction_data {
                            if let Some(instruction_data) = inner_tx_data {
                                print_task_sender.send(PrintTask { text_color: Color::DarkGreen, should_add_new_line: false, text: format!("✅ We fetched a instruction in tx with hash: {:?}", instruction_data.get_tx_hash()) }).await.map_err(|send_error| SyncCronError::Network(send_error.to_string()))?;
                                if is_this_first_element {
                                    is_this_first_element = false;
                                } else {
                                    streaming_file.write(b",").await?;
                                }
                                streaming_file.write(serde_json::to_string_pretty(&instruction_data).expect("Data to be serializable").as_bytes()).await?;
                            } else {
                                // No more tx
                                print_task_sender.send(PrintTask { text_color: Color::DarkGreen, should_add_new_line: true, text: format!("All Txs are written!") }).await.map_err(|send_error| SyncCronError::Network(send_error.to_string()))?;
                                break_loop = true;
                            }
                        } else {
                            break_loop = true;
                        }
                    }
            }
        }

        join!(tx_data_fetch_task).0.map_err(anyhow::Error::new)??;
        streaming_file.write(b"]").await?;

        printer_task.abort();

        return Ok(());
    }
}
