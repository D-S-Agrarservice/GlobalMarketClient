//! Backend service abstraction, local simulation mock, and FastAPI v2.0 HTTP client (`docs/openapi.json`).

pub mod fastapi;
pub mod mock;
pub mod types;

use crate::api::types::{
    MarketStateResponse, ServerHandshake, TransactionRequest, WhitelistSyncRequest,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Unauthorized (invalid X-Market-Token)")]
    Unauthorized,
    #[error("Key slot limit reached (max. {max_slots} active client slots)")]
    SlotLimitReached { max_slots: u32 },
    #[error("Client ID bound to another network IP (retry in {retry_after_sec}s)")]
    ClientIpLocked { retry_after_sec: u64 },
    #[error("Missing or invalid X-Client-Id header for market key")]
    ClientIdRequired,
    #[error("Internal error: {0}")]
    Internal(String),
}

pub trait BackendService: Send + Sync {
    fn check_status(&self) -> Result<ServerHandshake, ApiError>;
    fn sync_whitelist(&self, req: WhitelistSyncRequest) -> Result<MarketStateResponse, ApiError>;
    fn fetch_market_state(&self) -> Result<MarketStateResponse, ApiError>;
    fn submit_transaction(&self, tx: TransactionRequest) -> Result<MarketStateResponse, ApiError>;
    fn fetch_logo_bytes(&self, logo_url: &str) -> Result<Vec<u8>, ApiError>;
}
