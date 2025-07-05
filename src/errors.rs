use anchor_client::solana_client::client_error::ClientError;
use anchor_client::ClientError as AnchorClientError;
use serde_json;
use std::io;
use std::string::FromUtf8Error;
use thiserror::Error;
use tokio_cron_scheduler::JobSchedulerError;

#[derive(Debug, Error)]
pub enum SyncCronError {
    #[error("KV Database error: {0}")]
    Database(#[from] sled::Error),

    #[error("Solana client error: {0}")]
    SolanaClient(#[from] ClientError),

    #[error("Anchor client error: {0}")]
    AnchorClient(#[from] AnchorClientError),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Transaction failed: {0}")]
    TransactionFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Anyhow error: {0}")]
    Anyhow(#[from] anyhow::Error),

    #[error("BS58 decode error: {0}")]
    Bs58Decode(#[from] bs58::decode::Error),

    #[error("Parse pubkey error: {0}")]
    ParsePubkey(#[from] anchor_client::solana_sdk::pubkey::ParsePubkeyError),

    #[error("UTF8 conversion error: {0}")]
    Utf8(#[from] FromUtf8Error),

    #[error("Job scheduler error: {0}")]
    JobScheduler(#[from] JobSchedulerError),

    #[error("Network error: {0}")]
    Network(String),
}

pub type Result<T> = std::result::Result<T, SyncCronError>;
