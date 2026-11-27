//! Backend service abstraction, local simulation mock, and FastAPI v2.0 HTTP client (`docs/openapi.json`).

pub mod fastapi;
pub mod mock;
pub mod types;

use crate::api::types::{
    ContractActionRequest, ContractStateResponse, LoadedMap, MarketStateResponse, ServerHandshake,
    ServerSecret, TransactionRequest, VehicleActionRequest, VehicleStateResponse,
    WhitelistSyncRequest,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Endpoint not supported by backend: {0}")]
    NotSupported(String),
    #[error("Unauthorized (invalid X-Market-Token)")]
    Unauthorized,
    #[error("Key slot limit reached (max. {max_slots} active client slots)")]
    SlotLimitReached { max_slots: u32 },
    #[error("Client ID bound to another network IP (retry in {retry_after_sec}s)")]
    ClientIpLocked { retry_after_sec: u64 },
    #[error("Missing or invalid X-Client-Id header for market key")]
    ClientIdRequired,
    /// `HTTP 403` with code `SERVER_SECRET_INVALID` (Spec Section 3.5): the savegame is registered and
    /// `X-Server-Secret` is missing or wrong.
    #[error("Savegame identity rejected (X-Server-Secret)")]
    ServerSecretInvalid,
    /// `HTTP 403` with code `DEV_TOKEN_REQUIRED` (`missing`) or `DEV_TOKEN_INVALID` from a
    /// development server: every request there needs an active `X-Dev-Token`.
    #[error("Development server rejected the developer token (X-Dev-Token)")]
    DevTokenRejected { missing: bool },
    /// `HTTP 403` with code `CLIENT_BLOCKED`: the operators blocked this IP, client id or savegame.
    #[error("Blocked by the server operators")]
    ClientBlocked,
    /// `HTTP 403` with code `CONTENT_NOT_SUPPORTED`: the backend refuses the loaded map. `label` may
    /// name the content and is shown as is; the reason itself stays on the server.
    #[error("Map not supported by Global Market")]
    ContentNotSupported { label: Option<String> },
    /// `HTTP 503` with code `MAINTENANCE` (Spec Section 3.1.3).
    #[error("Server maintenance in progress")]
    Maintenance {
        notice_id: Option<String>,
        /// Expected end as UTC unix seconds (`until`, or derived from `Retry-After`).
        until: Option<u64>,
    },
    #[error("Internal error: {0}")]
    Internal(String),
}

pub trait BackendService: Send + Sync {
    fn check_status(&self) -> Result<ServerHandshake, ApiError>;
    fn sync_whitelist(&self, req: WhitelistSyncRequest) -> Result<MarketStateResponse, ApiError>;
    fn fetch_market_state(&self) -> Result<MarketStateResponse, ApiError>;
    fn submit_transaction(&self, tx: TransactionRequest) -> Result<MarketStateResponse, ApiError>;
    fn fetch_contracts_state(
        &self,
        server_id: Option<&str>,
        server_secret: Option<&ServerSecret>,
    ) -> Result<ContractStateResponse, ApiError>;
    fn submit_contract_action(
        &self,
        action: ContractActionRequest,
        server_secret: Option<&ServerSecret>,
    ) -> Result<ContractStateResponse, ApiError>;
    /// `GET /api/v1/vehicles/state` (Spec Section 3.6). Without `server_id` only the board.
    fn fetch_vehicles_state(
        &self,
        server_id: Option<&str>,
        server_secret: Option<&ServerSecret>,
    ) -> Result<VehicleStateResponse, ApiError>;
    /// `POST /api/v1/vehicles/actions` (Spec Section 3.6).
    fn submit_vehicle_action(
        &self,
        action: VehicleActionRequest,
        server_secret: Option<&ServerSecret>,
    ) -> Result<VehicleStateResponse, ApiError>;
    fn fetch_logo_bytes(&self, logo_url: &str) -> Result<Vec<u8>, ApiError>;
    /// Loaded map for the optional `X-Map-*` headers (Spec Section 3.1.1); `None` sends none.
    fn set_loaded_map(&self, _map: Option<LoadedMap>) {}
    /// Forgets the savegame identity that is sent as `X-Server-Id` + `X-Server-Secret` on every
    /// request (new map load, other market).
    fn forget_savegame_identity(&self) {}
}
