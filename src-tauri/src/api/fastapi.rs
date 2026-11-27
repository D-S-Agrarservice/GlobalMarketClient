//! Production HTTP client for the Python / FastAPI backend (`FS25: Global Market v2.0.0`).
//!
//! Aligned 1:1 with `docs/openapi.json` & `docs/Phase2_Contracts_Handoff_Client_and_Backend.md`:
//! - `GET  /api/v1/meta/status`         -> `MetaStatus` (including `contracts_supported`)
//! - `GET  /api/v1/market/state`        -> `200 MarketState` or `304 Not Modified` via `ETag` / `If-None-Match`
//! - `POST /api/v1/market/whitelist`    -> `WhitelistRequest` -> `200 MarketState`
//! - `POST /api/v1/market/transactions` -> `TransactionRequest` -> `200 MarketState`
//! - `GET  /api/v1/contracts/state`     -> `200 ContractState` or `304 Not Modified` (`?server_id=...`)
//! - `POST /api/v1/contracts/actions`   -> `ContractActionRequest` -> `200 ContractState`
//! - `GET  /api/v1/vehicles/state`      -> `200 VehicleState` or `304 Not Modified` (`?server_id=...`)
//! - `POST /api/v1/vehicles/actions`    -> `VehicleActionRequest` -> `200 VehicleState`
//! - Optional `X-Market-Token` header for private/community/logo keys (absent = official public market).
//! - Persistent random `X-Client-Id` header so the backend can enforce 16/32 active client slots per key without hardware tracking.

use crate::api::types::{
    ContractActionRequest, ContractActionResultDto, ContractSettlementDto, ContractStateResponse,
    FillTypeStatus, LoadedMap, MarketItemDto, MarketStateResponse, OnlineContractDto, OnlineDirectOfferDto,
    normalize_server_env, OnlineVehicleListingDto, ServerHandshake, ServerNotice, ServerSecret,
    TransactionRequest,
    VehicleActionRequest, VehicleActionResultDto, VehicleConfigDto, VehicleDto,
    VehicleSettlementDto, VehicleStateResponse, WhitelistSyncRequest,
};
use crate::api::{ApiError, BackendService};
use crate::bridge::dds::MAX_LOGO_RAW_BYTES;
use crate::bridge::xml::sanitize_market_name;
use reqwest::header::{ETAG, IF_NONE_MATCH, RETRY_AFTER};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::error::Error as StdError;
use std::sync::{Mutex, RwLock};
use std::time::Duration;

#[derive(Debug, Clone)]
struct CachedMarketState {
    etag: String,
    state: MarketStateResponse,
}

#[derive(Debug, Clone)]
struct CachedVehicleState {
    server_id: Option<String>,
    etag: String,
    state: VehicleStateResponse,
}

#[derive(Debug, Clone)]
struct CachedContractState {
    server_id: Option<String>,
    etag: String,
    state: ContractStateResponse,
}

pub struct FastApiBackend {
    base_url: RwLock<String>,
    market_token: RwLock<Option<String>>,
    client_id: RwLock<Option<String>>,
    /// Developer token (`X-Dev-Token`), sent only to development servers (see [`dev_token_allowed`]).
    dev_token: RwLock<Option<String>>,
    /// Loaded map from `modInfo.xml`, sent as `X-Map-Id` / `X-Map-Mod` / `X-Map-Hash`.
    loaded_map: RwLock<Option<LoadedMap>>,
    /// Last savegame identity that worked on a board request, sent as `X-Server-Id` +
    /// `X-Server-Secret` on every request, so a savegame on unsupported content can still settle.
    savegame_identity: RwLock<Option<(String, ServerSecret)>>,
    etag_cache: Mutex<Option<CachedMarketState>>,
    contract_etag_cache: Mutex<Option<CachedContractState>>,
    vehicle_etag_cache: Mutex<Option<CachedVehicleState>>,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}

impl FastApiBackend {
    pub fn new(
        base_url: String,
        market_token: Option<String>,
        client_id: String,
    ) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(format!(
                "GlobalMarketClient/{}",
                crate::PROTOCOL_EXE_VERSION
            ))
            .build()
            .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| format!("Failed to initialize async runtime for HTTP client: {e}"))?;

        Ok(Self {
            base_url: RwLock::new(normalize_base_url(&base_url)),
            market_token: RwLock::new(clean_token(market_token)),
            client_id: RwLock::new(clean_client_id(&client_id)),
            dev_token: RwLock::new(None),
            loaded_map: RwLock::new(None),
            savegame_identity: RwLock::new(None),
            etag_cache: Mutex::new(None),
            contract_etag_cache: Mutex::new(None),
            vehicle_etag_cache: Mutex::new(None),
            client,
            runtime,
        })
    }

    /// Updates target base URL, optional `X-Market-Token`, and `X-Client-Id`, clearing any cached
    /// ETag snapshot whenever the target endpoint or market scope changes.
    pub fn update_connection_config(
        &self,
        base_url: String,
        market_token: Option<String>,
        client_id: String,
    ) {
        let normalized_url = normalize_base_url(&base_url);
        let cleaned_token = clean_token(market_token);
        let cleaned_cid = clean_client_id(&client_id);

        let mut changed = false;
        if let Ok(mut guard) = self.base_url.write() {
            if *guard != normalized_url {
                *guard = normalized_url;
                changed = true;
            }
        }
        if let Ok(mut guard) = self.market_token.write() {
            if *guard != cleaned_token {
                *guard = cleaned_token;
                changed = true;
            }
        }
        if let Ok(mut guard) = self.client_id.write() {
            if *guard != cleaned_cid {
                *guard = cleaned_cid;
                changed = true;
            }
        }

        if changed {
            self.clear_etag_cache();
            self.clear_savegame_identity();
        }
    }

    fn clear_savegame_identity(&self) {
        if let Ok(mut guard) = self.savegame_identity.write() {
            *guard = None;
        }
    }

    /// Remembers the identity of a successful board request (an issued secret wins over the sent one).
    fn remember_savegame_identity(&self, server_id: &str, secret: Option<&ServerSecret>) {
        let sid = server_id.trim();
        let Some(secret) = secret else { return };
        if sid.is_empty() || reqwest::header::HeaderValue::from_str(sid).is_err() {
            return;
        }
        if let Ok(mut guard) = self.savegame_identity.write() {
            *guard = Some((sid.to_string(), secret.clone()));
        }
    }

    /// Sets or clears the developer token for `X-Dev-Token`.
    pub fn set_dev_token(&self, dev_token: Option<String>) {
        let cleaned = clean_token(dev_token);
        if let Ok(mut guard) = self.dev_token.write() {
            if *guard != cleaned {
                *guard = cleaned;
                drop(guard);
                self.clear_etag_cache();
            }
        }
    }

    pub fn clear_etag_cache(&self) {
        if let Ok(mut guard) = self.etag_cache.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.contract_etag_cache.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.vehicle_etag_cache.lock() {
            *guard = None;
        }
    }

    fn store_or_clear_vehicle_etag(
        &self,
        server_id: Option<String>,
        etag_opt: Option<String>,
        state: &VehicleStateResponse,
    ) {
        if let Ok(mut guard) = self.vehicle_etag_cache.lock() {
            *guard = match etag_opt {
                Some(etag) if !etag.is_empty() => Some(CachedVehicleState {
                    server_id,
                    etag,
                    state: VehicleStateResponse {
                        server_secret: None,
                        ..state.clone()
                    },
                }),
                _ => None,
            };
        }
    }

    fn store_or_clear_etag(&self, etag_opt: Option<String>, state: &MarketStateResponse) {
        if let Ok(mut guard) = self.etag_cache.lock() {
            match etag_opt {
                Some(etag) if !etag.is_empty() => {
                    *guard = Some(CachedMarketState {
                        etag,
                        state: state.clone(),
                    });
                }
                _ => {
                    *guard = None;
                }
            }
        }
    }

    fn store_or_clear_contract_etag(
        &self,
        server_id: Option<String>,
        etag_opt: Option<String>,
        state: &ContractStateResponse,
    ) {
        if let Ok(mut guard) = self.contract_etag_cache.lock() {
            match etag_opt {
                Some(etag) if !etag.is_empty() => {
                    *guard = Some(CachedContractState {
                        server_id,
                        etag,
                        state: state.clone(),
                    });
                }
                _ => {
                    *guard = None;
                }
            }
        }
    }

    fn endpoint(&self, suffix: &str) -> String {
        let base = self
            .base_url
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| crate::config::DEFAULT_OFFICIAL_BACKEND_URL.to_string());
        format!("{base}{suffix}")
    }

    /// Common headers plus the remembered savegame identity (`X-Server-Id` + `X-Server-Secret`).
    fn apply_auth_header(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let req = self.apply_common_headers(req);
        match self.savegame_identity.read().ok().and_then(|g| g.clone()) {
            Some((sid, secret)) => apply_server_secret(req.header("X-Server-Id", sid), Some(&secret)),
            None => req,
        }
    }

    /// Headers of a board request for savegame `server_id`: its own identity instead of the remembered one.
    fn apply_board_headers(
        &self,
        req: reqwest::RequestBuilder,
        server_id: &str,
        server_secret: Option<&ServerSecret>,
    ) -> reqwest::RequestBuilder {
        let mut req = self.apply_common_headers(req);
        if server_secret.is_some() {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(server_id.trim()) {
                req = req.header("X-Server-Id", value);
            }
        }
        apply_server_secret(req, server_secret)
    }

    fn apply_common_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(cid) = self.client_id.read().ok().and_then(|g| g.clone()) {
            req = req.header("X-Client-Id", cid);
        }
        if let Some(value) = self.dev_token_header() {
            req = req.header("X-Dev-Token", value);
        }
        if let Some(map) = self.loaded_map.read().ok().and_then(|g| g.clone()) {
            req = req.header("X-Map-Id", map.map_id);
            if let Some(name) = map.map_mod_name {
                req = req.header("X-Map-Mod", name);
            }
            if let Some(hash) = map.map_hash {
                req = req.header("X-Map-Hash", hash);
            }
        }
        let token = self.market_token.read().ok().and_then(|g| g.clone());
        match token {
            Some(t) => req.header("X-Market-Token", t),
            None => req,
        }
    }

    /// `X-Dev-Token` (marked sensitive) when a token is set and the base URL is a development server.
    fn dev_token_header(&self) -> Option<reqwest::header::HeaderValue> {
        let base_url = self.base_url.read().ok()?.clone();
        if !dev_token_allowed(&base_url) {
            return None;
        }
        let token = self.dev_token.read().ok()?.clone()?;
        let mut value = reqwest::header::HeaderValue::from_str(&token).ok()?;
        value.set_sensitive(true);
        Some(value)
    }
}

/// Adds `X-Server-Secret` (marked sensitive, so it never shows up in debug output).
fn apply_server_secret(
    req: reqwest::RequestBuilder,
    server_secret: Option<&ServerSecret>,
) -> reqwest::RequestBuilder {
    match server_secret.and_then(|s| reqwest::header::HeaderValue::from_str(s.expose()).ok()) {
        Some(mut value) => {
            value.set_sensitive(true);
            req.header("X-Server-Secret", value)
        }
        None => req,
    }
}

#[derive(Debug, Deserialize)]
struct WireMetaStatus {
    #[serde(default = "default_true")]
    online: bool,
    #[serde(default = "default_min_ver")]
    min_exe_version: String,
    #[serde(
        default,
        alias = "recommended_exe_version",
        alias = "recommended_version",
        alias = "latest_version"
    )]
    latest_exe_version: Option<String>,
    #[serde(default = "default_poll")]
    poll_interval_sec: u64,
    #[serde(default = "default_market_name")]
    market_name: String,
    #[serde(default = "default_api_ver")]
    api_version: String,
    #[serde(default, alias = "tier")]
    branding_tier: Option<u8>,
    #[serde(default, alias = "private_market")]
    is_private_market: Option<bool>,
    #[serde(default)]
    has_custom_logo: Option<bool>,
    #[serde(default, alias = "logo_url")]
    market_logo_url: Option<String>,
    #[serde(default, alias = "contractsSupported")]
    contracts_supported: bool,
    #[serde(default, alias = "vehiclesSupported")]
    vehicles_supported: bool,
    /// Parsed leniently via [`ServerNotice::from_wire`], so a malformed notice never fails the handshake.
    #[serde(default)]
    notice: Option<serde_json::Value>,
    #[serde(default)]
    server_env: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}
fn default_min_ver() -> String {
    "0.9.9".to_string()
}
fn default_poll() -> u64 {
    10
}
fn default_market_name() -> String {
    "Global Market".to_string()
}
fn default_api_ver() -> String {
    "2.0.0".to_string()
}

#[derive(Debug, Serialize)]
struct WireWhitelistItem<'a> {
    name: &'a str,
    title: &'a str,
    category: &'a str,
    #[serde(rename = "type")]
    item_type: &'a str,
    base_price: f64,
    source: &'a str,
}

#[derive(Debug, Serialize)]
struct WireWhitelistPayload<'a> {
    lang: &'a str,
    items: Vec<WireWhitelistItem<'a>>,
}

#[derive(Debug, Serialize)]
struct WireTransactionPayload<'a> {
    tx_id: &'a str,
    fill_type: &'a str,
    amount: i64,
    #[serde(rename = "type")]
    tx_type: &'a str,
}

#[derive(Debug, Deserialize)]
struct WireMarketState {
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    items: Vec<WireMarketItem>,
}

#[derive(Debug, Deserialize)]
struct WireMarketItem {
    fill_type: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    fill_level: i64,
    #[serde(default = "default_one")]
    trend: f32,
    #[serde(default)]
    great_demand: bool,
    #[serde(default)]
    great_demand_bonus: f32,
    #[serde(default)]
    great_demand_remaining: i64,
    #[serde(default)]
    status: String,
}

fn default_one() -> f32 {
    1.0
}

fn default_origin_online() -> String {
    "ONLINE".to_string()
}

fn default_status_open() -> String {
    "OPEN".to_string()
}

#[derive(Debug, Serialize)]
struct WireContractActionPayload<'a> {
    sub_action: &'a str,
    action_id: &'a str,
    server_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    farm_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    farm_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fill_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    liters: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_trend_factor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bonus_pct: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_delta_pct: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_partial: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_hours: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    was_bale: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    was_pallet: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settlement_ids: Option<&'a str>,
}

#[derive(Deserialize)]
struct WireContractState {
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    contracts: Vec<WireOnlineContract>,
    #[serde(default, alias = "directOffers")]
    direct_offers: Vec<WireOnlineOffer>,
    #[serde(default, alias = "actionResults")]
    action_results: Vec<WireContractActionResult>,
    #[serde(default)]
    settlements: Vec<WireContractSettlement>,
    #[serde(default, alias = "serverSecret")]
    server_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireOnlineContract {
    #[serde(alias = "remoteId", alias = "id")]
    remote_id: String,
    #[serde(default = "default_origin_online")]
    origin: String,
    #[serde(default, alias = "isNpc")]
    is_npc: bool,
    #[serde(default, alias = "issuerServerId")]
    issuer_server_id: String,
    #[serde(default, alias = "issuerFarmId")]
    issuer_farm_id: i32,
    #[serde(default, alias = "issuerName")]
    issuer_name: String,
    #[serde(default, alias = "contractorServerId")]
    contractor_server_id: String,
    #[serde(default, alias = "contractorFarmId")]
    contractor_farm_id: i32,
    #[serde(alias = "fillType")]
    fill_type: String,
    #[serde(default, alias = "totalLiters")]
    total_liters: i64,
    #[serde(default, alias = "deliveredLiters")]
    delivered_liters: i64,
    #[serde(default = "default_one", alias = "baseTrendFactor")]
    base_trend_factor: f32,
    #[serde(default, alias = "bonusPct")]
    bonus_pct: i32,
    #[serde(default, alias = "remainingSeconds")]
    remaining_seconds: i64,
    #[serde(default = "default_status_open")]
    status: String,
}

#[derive(Debug, Deserialize)]
struct WireOnlineOffer {
    #[serde(alias = "remoteId", alias = "id")]
    remote_id: String,
    #[serde(default = "default_origin_online")]
    origin: String,
    #[serde(default, alias = "isNpc")]
    is_npc: bool,
    #[serde(default, alias = "sellerServerId")]
    seller_server_id: String,
    #[serde(default, alias = "sellerFarmId")]
    seller_farm_id: i32,
    #[serde(default, alias = "sellerName")]
    seller_name: String,
    #[serde(alias = "fillType")]
    fill_type: String,
    #[serde(default, alias = "totalLiters")]
    total_liters: i64,
    #[serde(default, alias = "remainingLiters")]
    remaining_liters: i64,
    #[serde(default, alias = "priceDeltaPct")]
    price_delta_pct: i32,
    #[serde(default = "default_true", alias = "allowPartial")]
    allow_partial: bool,
    #[serde(default, alias = "remainingSeconds")]
    remaining_seconds: i64,
    #[serde(default, alias = "wasBale")]
    was_bale: bool,
    #[serde(default, alias = "wasPallet")]
    was_pallet: bool,
    #[serde(default = "default_status_open")]
    status: String,
}

#[derive(Debug, Deserialize)]
struct WireContractActionResult {
    #[serde(alias = "actionId")]
    action_id: String,
    #[serde(default, alias = "subAction")]
    sub_action: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    reason: String,
    #[serde(default, alias = "remoteId")]
    remote_id: String,
    #[serde(default, alias = "grantedLiters")]
    granted_liters: i64,
    #[serde(default = "default_one", alias = "baseTrendFactor")]
    base_trend_factor: f32,
    #[serde(default, alias = "bonusPct")]
    bonus_pct: i32,
    #[serde(default, alias = "remainingSeconds")]
    remaining_seconds: i64,
}

#[derive(Debug, Deserialize)]
struct WireContractSettlement {
    #[serde(alias = "settlementId", alias = "id")]
    settlement_id: String,
    #[serde(default, rename = "type", alias = "settlementType")]
    settlement_type: String,
    #[serde(default, alias = "targetServerId")]
    target_server_id: String,
    #[serde(default, alias = "targetFarmId")]
    target_farm_id: i32,
    #[serde(default, alias = "remoteId")]
    remote_id: String,
    #[serde(default, alias = "fillType")]
    fill_type: String,
    #[serde(default)]
    liters: i64,
    #[serde(default, alias = "missingLiters")]
    missing_liters: i64,
    #[serde(default = "default_one", alias = "baseTrendFactor")]
    base_trend_factor: f32,
    #[serde(default, alias = "priceDeltaPct")]
    price_delta_pct: i32,
    #[serde(default, alias = "sourceLabel")]
    source_label: String,
}

impl From<WireContractActionResult> for ContractActionResultDto {
    fn from(r: WireContractActionResult) -> Self {
        Self {
            action_id: r.action_id.trim().to_string(),
            sub_action: r.sub_action.trim().to_ascii_uppercase(),
            status: if r.status.trim().is_empty() {
                "REJECTED".to_string()
            } else {
                r.status.trim().to_ascii_uppercase()
            },
            reason: r.reason,
            remote_id: r.remote_id.trim().to_string(),
            granted_liters: r.granted_liters.max(0),
            base_trend_factor: if r.base_trend_factor.is_finite() && r.base_trend_factor > 0.0 {
                r.base_trend_factor
            } else {
                1.0
            },
            bonus_pct: r.bonus_pct,
            remaining_seconds: r.remaining_seconds.max(0),
        }
    }
}

impl From<WireContractState> for ContractStateResponse {
    fn from(wire: WireContractState) -> Self {
        Self {
            updated_at: wire.updated_at,
            server_secret: wire.server_secret.as_deref().and_then(ServerSecret::parse),
            contracts: wire
                .contracts
                .into_iter()
                .filter(|c| !c.remote_id.trim().is_empty() && !c.fill_type.trim().is_empty())
                .map(|c| OnlineContractDto {
                    remote_id: c.remote_id.trim().to_string(),
                    origin: if c.origin.trim().is_empty() {
                        "ONLINE".to_string()
                    } else {
                        c.origin.trim().to_ascii_uppercase()
                    },
                    is_npc: c.is_npc,
                    issuer_server_id: c.issuer_server_id.trim().to_string(),
                    issuer_farm_id: c.issuer_farm_id.max(0),
                    issuer_name: if c.issuer_name.trim().is_empty() {
                        "Online".to_string()
                    } else {
                        c.issuer_name
                    },
                    contractor_server_id: c.contractor_server_id.trim().to_string(),
                    contractor_farm_id: c.contractor_farm_id.max(0),
                    fill_type: c.fill_type.trim().to_ascii_uppercase(),
                    total_liters: c.total_liters.max(0),
                    delivered_liters: c.delivered_liters.max(0),
                    base_trend_factor: if c.base_trend_factor.is_finite()
                        && c.base_trend_factor > 0.0
                    {
                        c.base_trend_factor
                    } else {
                        1.0
                    },
                    bonus_pct: c.bonus_pct,
                    remaining_seconds: c.remaining_seconds.max(0),
                    status: if c.status.trim().is_empty() {
                        "OPEN".to_string()
                    } else {
                        c.status.trim().to_ascii_uppercase()
                    },
                })
                .collect(),
            direct_offers: wire
                .direct_offers
                .into_iter()
                .filter(|o| !o.remote_id.trim().is_empty() && !o.fill_type.trim().is_empty())
                .map(|o| OnlineDirectOfferDto {
                    remote_id: o.remote_id.trim().to_string(),
                    origin: if o.origin.trim().is_empty() {
                        "ONLINE".to_string()
                    } else {
                        o.origin.trim().to_ascii_uppercase()
                    },
                    is_npc: o.is_npc,
                    seller_server_id: o.seller_server_id.trim().to_string(),
                    seller_farm_id: o.seller_farm_id.max(0),
                    seller_name: if o.seller_name.trim().is_empty() {
                        "Online".to_string()
                    } else {
                        o.seller_name
                    },
                    fill_type: o.fill_type.trim().to_ascii_uppercase(),
                    total_liters: o.total_liters.max(0),
                    remaining_liters: o.remaining_liters.max(0),
                    price_delta_pct: o.price_delta_pct,
                    allow_partial: o.allow_partial,
                    remaining_seconds: o.remaining_seconds.max(0),
                    was_bale: o.was_bale,
                    was_pallet: o.was_pallet,
                    status: if o.status.trim().is_empty() {
                        "OPEN".to_string()
                    } else {
                        o.status.trim().to_ascii_uppercase()
                    },
                })
                .collect(),
            action_results: wire
                .action_results
                .into_iter()
                .filter(|r| !r.action_id.trim().is_empty())
                .map(ContractActionResultDto::from)
                .collect(),
            settlements: wire
                .settlements
                .into_iter()
                .filter(|s| !s.settlement_id.trim().is_empty())
                .map(|s| ContractSettlementDto {
                    settlement_id: s.settlement_id.trim().to_string(),
                    settlement_type: s.settlement_type.trim().to_ascii_uppercase(),
                    target_server_id: s.target_server_id.trim().to_string(),
                    target_farm_id: s.target_farm_id.max(0),
                    remote_id: s.remote_id.trim().to_string(),
                    fill_type: s.fill_type.trim().to_ascii_uppercase(),
                    liters: s.liters.max(0),
                    missing_liters: s.missing_liters.max(0),
                    base_trend_factor: if s.base_trend_factor.is_finite()
                        && s.base_trend_factor > 0.0
                    {
                        s.base_trend_factor
                    } else {
                        1.0
                    },
                    price_delta_pct: s.price_delta_pct,
                    source_label: s.source_label,
                })
                .collect(),
        }
    }
}

/// Integer field that tolerates a JSON float (`26.0`) or a numeric string.
fn de_i64_lenient<'de, D: serde::Deserializer<'de>>(de: D) -> Result<i64, D::Error> {
    let value = serde_json::Value::deserialize(de)?;
    Ok(match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64))
            .unwrap_or(0),
        serde_json::Value::String(s) => s.trim().parse::<f64>().map_or(0, |f| f.round() as i64),
        _ => 0,
    })
}

/// String field that tolerates a number (`save_id: 3`) or `null`.
fn de_string_lenient<'de, D: serde::Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    let value = serde_json::Value::deserialize(de)?;
    Ok(match value {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => String::new(),
    })
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct WireVehicleConfig {
    #[serde(default, deserialize_with = "de_string_lenient")]
    name: String,
    #[serde(default, alias = "saveId", deserialize_with = "de_string_lenient")]
    save_id: String,
    #[serde(default, alias = "isActive")]
    is_active: bool,
    #[serde(default, alias = "isBought")]
    is_bought: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<Vec<f64>>,
    #[serde(default, alias = "materialTemplate", skip_serializing_if = "Option::is_none")]
    material_template: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireVehicle {
    #[serde(default, alias = "xmlFilename", deserialize_with = "de_string_lenient")]
    xml_filename: String,
    #[serde(default, alias = "modName", skip_serializing_if = "Option::is_none")]
    mod_name: Option<String>,
    #[serde(default, alias = "modFileHash", skip_serializing_if = "Option::is_none")]
    mod_file_hash: Option<String>,
    #[serde(default, deserialize_with = "de_string_lenient")]
    group: String,
    #[serde(default, alias = "ageMonths", deserialize_with = "de_i64_lenient")]
    age_months: i64,
    #[serde(default, alias = "operatingHours")]
    operating_hours: f64,
    #[serde(default)]
    damage: f64,
    #[serde(default)]
    wear: f64,
    /// `null` in listings of a `compact=true` board.
    #[serde(default)]
    configurations: Option<Vec<WireVehicleConfig>>,
}

fn clean_opt_text(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl From<WireVehicle> for VehicleDto {
    fn from(v: WireVehicle) -> Self {
        Self {
            xml_filename: v.xml_filename.trim().to_string(),
            mod_name: clean_opt_text(v.mod_name),
            mod_file_hash: clean_opt_text(v.mod_file_hash),
            group: v.group.trim().to_string(),
            age_months: v.age_months.max(0),
            operating_hours: finite_or(v.operating_hours, 0.0).max(0.0),
            damage: finite_or(v.damage, 0.0).clamp(0.0, 1.0),
            wear: finite_or(v.wear, 0.0).clamp(0.0, 1.0),
            configurations: v
                .configurations
                .unwrap_or_default()
                .into_iter()
                .map(|c| VehicleConfigDto {
                    name: c.name.trim().to_string(),
                    save_id: c.save_id.trim().to_string(),
                    is_active: c.is_active,
                    is_bought: c.is_bought,
                    color: c
                        .color
                        .filter(|rgb| rgb.len() == 3 && rgb.iter().all(|f| f.is_finite()))
                        .map(|rgb| [rgb[0], rgb[1], rgb[2]]),
                    material_template: c.material_template,
                })
                .collect(),
        }
    }
}

impl From<&VehicleDto> for WireVehicle {
    fn from(v: &VehicleDto) -> Self {
        Self {
            xml_filename: v.xml_filename.clone(),
            mod_name: v.mod_name.clone(),
            mod_file_hash: v.mod_file_hash.clone(),
            group: v.group.clone(),
            age_months: v.age_months,
            operating_hours: v.operating_hours,
            damage: v.damage,
            wear: v.wear,
            configurations: Some(
                v.configurations
                    .iter()
                    .map(|c| WireVehicleConfig {
                        name: c.name.clone(),
                        save_id: c.save_id.clone(),
                        is_active: c.is_active,
                        is_bought: c.is_bought,
                        color: c.color.map(|rgb| rgb.to_vec()),
                        material_template: c.material_template.clone(),
                    })
                    .collect(),
            ),
        }
    }
}

#[derive(Debug, Serialize)]
struct WireVehicleActionPayload<'a> {
    sub_action: &'a str,
    action_id: &'a str,
    server_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    farm_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    farm_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    create_action_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sale_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_factor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_hours: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settlement_ids: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vehicle: Option<WireVehicle>,
}

#[derive(Debug, Deserialize)]
struct WireVehicleListing {
    #[serde(alias = "remoteId", alias = "id", deserialize_with = "de_string_lenient")]
    remote_id: String,
    #[serde(default, alias = "createActionId", deserialize_with = "de_string_lenient")]
    create_action_id: String,
    #[serde(default, alias = "saleMode", deserialize_with = "de_string_lenient")]
    sale_mode: String,
    #[serde(default, alias = "sellerServerId", deserialize_with = "de_string_lenient")]
    seller_server_id: String,
    #[serde(default, alias = "sellerFarmId", deserialize_with = "de_i64_lenient")]
    seller_farm_id: i64,
    #[serde(default, alias = "sellerName", deserialize_with = "de_string_lenient")]
    seller_name: String,
    #[serde(default = "default_one_f64", alias = "priceFactor")]
    price_factor: f64,
    #[serde(default, alias = "remainingSeconds", deserialize_with = "de_i64_lenient")]
    remaining_seconds: i64,
    #[serde(default, deserialize_with = "de_string_lenient")]
    status: String,
    #[serde(default)]
    vehicle: Option<WireVehicle>,
}

#[derive(Debug, Deserialize)]
struct WireVehicleActionResult {
    #[serde(alias = "actionId", deserialize_with = "de_string_lenient")]
    action_id: String,
    #[serde(default, alias = "subAction", deserialize_with = "de_string_lenient")]
    sub_action: String,
    #[serde(default, deserialize_with = "de_string_lenient")]
    status: String,
    #[serde(default, deserialize_with = "de_string_lenient")]
    reason: String,
    #[serde(default, alias = "remoteId", deserialize_with = "de_string_lenient")]
    remote_id: String,
    #[serde(default, alias = "createActionId", deserialize_with = "de_string_lenient")]
    create_action_id: String,
    #[serde(default, alias = "remainingSeconds", deserialize_with = "de_i64_lenient")]
    remaining_seconds: i64,
    #[serde(default)]
    vehicle: Option<WireVehicle>,
}

#[derive(Debug, Deserialize)]
struct WireVehicleSettlement {
    #[serde(alias = "settlementId", alias = "id", deserialize_with = "de_string_lenient")]
    settlement_id: String,
    #[serde(default, rename = "type", alias = "settlementType", deserialize_with = "de_string_lenient")]
    settlement_type: String,
    #[serde(default, alias = "targetServerId", deserialize_with = "de_string_lenient")]
    target_server_id: String,
    #[serde(default, alias = "targetFarmId", deserialize_with = "de_i64_lenient")]
    target_farm_id: i64,
    #[serde(default, alias = "remoteId", deserialize_with = "de_string_lenient")]
    remote_id: String,
    #[serde(default, alias = "createActionId", deserialize_with = "de_string_lenient")]
    create_action_id: String,
    #[serde(default, alias = "buyActionId", deserialize_with = "de_string_lenient")]
    buy_action_id: String,
    #[serde(default = "default_one_f64", alias = "priceFactor")]
    price_factor: f64,
    #[serde(default, alias = "buyerName", deserialize_with = "de_string_lenient")]
    buyer_name: String,
    #[serde(default, alias = "sellerName", deserialize_with = "de_string_lenient")]
    seller_name: String,
    #[serde(default)]
    vehicle: Option<WireVehicle>,
}

#[derive(Deserialize)]
struct WireVehicleState {
    #[serde(default, deserialize_with = "de_i64_lenient")]
    updated_at: i64,
    #[serde(default)]
    listings: Vec<WireVehicleListing>,
    #[serde(default, alias = "actionResults")]
    action_results: Vec<WireVehicleActionResult>,
    #[serde(default)]
    settlements: Vec<WireVehicleSettlement>,
    #[serde(default, alias = "serverSecret")]
    server_secret: Option<String>,
}

fn default_one_f64() -> f64 {
    1.0
}

fn clamp_farm_id(raw: i64) -> i32 {
    raw.clamp(0, i32::MAX as i64) as i32
}

fn upper_or(raw: &str, fallback: &str) -> String {
    let clean = raw.trim();
    if clean.is_empty() {
        fallback.to_string()
    } else {
        clean.to_ascii_uppercase()
    }
}

impl From<WireVehicleState> for VehicleStateResponse {
    fn from(wire: WireVehicleState) -> Self {
        Self {
            updated_at: wire.updated_at,
            server_secret: wire.server_secret.as_deref().and_then(ServerSecret::parse),
            // Listings without a vehicle are useless for the mod; masked `""` ids stay as they are.
            listings: wire
                .listings
                .into_iter()
                .filter(|l| !l.remote_id.trim().is_empty())
                .filter_map(|l| {
                    let vehicle = VehicleDto::from(l.vehicle?);
                    Some(OnlineVehicleListingDto {
                        remote_id: l.remote_id.trim().to_string(),
                        create_action_id: l.create_action_id.trim().to_string(),
                        sale_mode: upper_or(&l.sale_mode, "FIXED"),
                        seller_server_id: l.seller_server_id.trim().to_string(),
                        seller_farm_id: clamp_farm_id(l.seller_farm_id),
                        seller_name: l.seller_name.trim().to_string(),
                        price_factor: finite_or(l.price_factor, 1.0),
                        remaining_seconds: l.remaining_seconds.max(0),
                        status: upper_or(&l.status, "OPEN"),
                        vehicle,
                    })
                })
                .collect(),
            action_results: wire
                .action_results
                .into_iter()
                .filter(|r| !r.action_id.trim().is_empty())
                .map(|r| VehicleActionResultDto {
                    action_id: r.action_id.trim().to_string(),
                    sub_action: r.sub_action.trim().to_ascii_uppercase(),
                    status: upper_or(&r.status, "REJECTED"),
                    reason: r.reason.trim().to_string(),
                    remote_id: r.remote_id.trim().to_string(),
                    create_action_id: r.create_action_id.trim().to_string(),
                    remaining_seconds: r.remaining_seconds.max(0),
                    vehicle: r.vehicle.map(VehicleDto::from),
                })
                .collect(),
            settlements: wire
                .settlements
                .into_iter()
                .filter(|s| !s.settlement_id.trim().is_empty())
                .map(|s| VehicleSettlementDto {
                    settlement_id: s.settlement_id.trim().to_string(),
                    settlement_type: s.settlement_type.trim().to_ascii_uppercase(),
                    target_server_id: s.target_server_id.trim().to_string(),
                    target_farm_id: clamp_farm_id(s.target_farm_id),
                    remote_id: s.remote_id.trim().to_string(),
                    create_action_id: s.create_action_id.trim().to_string(),
                    buy_action_id: s.buy_action_id.trim().to_string(),
                    price_factor: finite_or(s.price_factor, 1.0),
                    buyer_name: s.buyer_name.trim().to_string(),
                    seller_name: s.seller_name.trim().to_string(),
                    vehicle: s.vehicle.map(VehicleDto::from),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WireHttpValidationError {
    #[serde(default)]
    detail: Vec<WireValidationDetail>,
}

#[derive(Debug, Deserialize)]
struct WireValidationDetail {
    #[serde(default)]
    loc: Vec<serde_json::Value>,
    #[serde(default)]
    msg: String,
}

impl From<WireMarketState> for MarketStateResponse {
    fn from(wire: WireMarketState) -> Self {
        Self {
            updated_at: wire.updated_at,
            items: wire
                .items
                .into_iter()
                .map(|i| MarketItemDto {
                    fill_type: i.fill_type.trim().to_ascii_uppercase(),
                    title: i.title,
                    fill_level: i.fill_level,
                    trend: i.trend,
                    great_demand: i.great_demand,
                    great_demand_bonus: i.great_demand_bonus,
                    great_demand_remaining: i.great_demand_remaining,
                    status: if i.status.is_empty() {
                        FillTypeStatus::Approved
                    } else {
                        FillTypeStatus::from_wire_str(&i.status)
                    },
                })
                .collect(),
        }
    }
}

fn issued_secret_from_json(root: &serde_json::Value) -> Option<ServerSecret> {
    root.get("server_secret")
        .and_then(|v| v.as_str())
        .and_then(ServerSecret::parse)
}

fn extract_etag(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn format_reqwest_error(err: &reqwest::Error) -> String {
    let mut msg = err.to_string();
    let mut current = err.source();
    while let Some(cause) = current {
        let cause_str = cause.to_string();
        if !cause_str.is_empty() && !msg.contains(&cause_str) {
            msg.push_str(": ");
            msg.push_str(&cause_str);
        }
        current = cause.source();
    }
    msg
}

async fn map_error_response(resp: reqwest::Response, treat_4xx_as_validation: bool) -> ApiError {
    let status = resp.status();
    if status == StatusCode::SERVICE_UNAVAILABLE {
        let retry_after_hdr = resp
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());
        let body = resp.bytes().await.ok();
        return body
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|json_val| parse_maintenance_error(&json_val, retry_after_hdr))
            .unwrap_or_else(|| ApiError::Network(format!("HTTP {status}")));
    }
    if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::BAD_REQUEST
    {
        let retry_after_hdr = resp
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());
        if let Ok(body_bytes) = resp.bytes().await {
            if let Ok(json_val) = serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                if let Some(classified) = parse_classified_auth_error(&json_val, retry_after_hdr) {
                    return classified;
                }
            }
        }
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return ApiError::Unauthorized;
        }
        if treat_4xx_as_validation && status.is_client_error() {
            return ApiError::Validation(format!("HTTP {status}"));
        }
        return ApiError::Network(format!("HTTP {status}"));
    }
    if status == StatusCode::UNPROCESSABLE_ENTITY {
        if let Ok(parsed) = resp.json::<WireHttpValidationError>().await {
            if !parsed.detail.is_empty() {
                let joined = parsed
                    .detail
                    .into_iter()
                    .map(|d| {
                        let path = d
                            .loc
                            .into_iter()
                            .map(|v| match v {
                                serde_json::Value::String(s) => s,
                                other => other.to_string(),
                            })
                            .collect::<Vec<_>>()
                            .join(".");
                        if path.is_empty() {
                            d.msg
                        } else {
                            format!("{path}: {}", d.msg)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                return ApiError::Validation(format!("HTTP 422 ({joined})"));
            }
        }
        return ApiError::Validation("HTTP 422 Unprocessable Entity".to_string());
    }
    if treat_4xx_as_validation && status.is_client_error() {
        return ApiError::Validation(format!("HTTP {status}"));
    }
    ApiError::Network(format!("HTTP {status}"))
}

impl BackendService for FastApiBackend {
    fn check_status(&self) -> Result<ServerHandshake, ApiError> {
        let base_url = self
            .base_url
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| crate::config::DEFAULT_OFFICIAL_BACKEND_URL.to_string());
        let has_token = self
            .market_token
            .read()
            .ok()
            .and_then(|g| g.clone())
            .is_some();
        let url = format!("{base_url}/api/v1/meta/status");
        let req = self.apply_auth_header(self.client.get(url));
        let result = self.runtime.block_on(async move {
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;
            if !resp.status().is_success() {
                return Err(map_error_response(resp, false).await);
            }
            let parsed: WireMetaStatus = resp
                .json()
                .await
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            let resolved_logo =
                resolve_safe_logo_url(parsed.market_logo_url, &base_url, has_token);
            let (branding_tier, is_private_market, market_name, market_logo_url) =
                derive_handshake_branding(
                    has_token,
                    &parsed.market_name,
                    resolved_logo,
                    parsed.branding_tier,
                    parsed.is_private_market,
                    parsed.has_custom_logo,
                );
            Ok(ServerHandshake {
                online: parsed.online,
                min_exe_version: parsed.min_exe_version,
                latest_exe_version: clean_version_opt(parsed.latest_exe_version),
                poll_interval_sec: parsed.poll_interval_sec.max(2),
                market_name,
                api_version: parsed.api_version,
                branding_tier,
                is_private_market,
                market_logo_url,
                contracts_supported: parsed.contracts_supported,
                vehicles_supported: parsed.vehicles_supported,
                notice: parsed.notice.as_ref().and_then(ServerNotice::from_wire),
                server_env: normalize_server_env(
                    parsed.server_env.as_ref().and_then(|v| v.as_str()),
                ),
            })
        });
        // A remembered identity the backend no longer accepts must not keep every request failing.
        if matches!(result, Err(ApiError::ServerSecretInvalid)) {
            self.clear_savegame_identity();
        }
        result
    }

    fn sync_whitelist(
        &self,
        req_data: WhitelistSyncRequest,
    ) -> Result<MarketStateResponse, ApiError> {
        let url = self.endpoint("/api/v1/market/whitelist");
        let payload = WireWhitelistPayload {
            lang: &req_data.lang,
            items: req_data
                .items
                .iter()
                .filter(|i| !i.name.trim().is_empty())
                .map(|i| WireWhitelistItem {
                    name: i.name.trim(),
                    title: &i.title,
                    category: &i.category,
                    item_type: &i.item_type,
                    base_price: ((i.base_price as f64) * 1_000_000.0).round() / 1_000_000.0,
                    source: &i.source,
                })
                .collect(),
        };
        let req = self.apply_auth_header(self.client.post(url).json(&payload));
        let (etag_opt, state) = self.runtime.block_on(async move {
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;
            if !resp.status().is_success() {
                return Err(map_error_response(resp, true).await);
            }
            let etag = extract_etag(resp.headers());
            let wire: WireMarketState = resp
                .json()
                .await
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            Ok((etag, MarketStateResponse::from(wire)))
        })?;

        // Update or invalidate the ETag snapshot so a subsequent GET /market/state never returns pre-whitelist data.
        self.store_or_clear_etag(etag_opt, &state);
        Ok(state)
    }

    fn fetch_market_state(&self) -> Result<MarketStateResponse, ApiError> {
        let url = self.endpoint("/api/v1/market/state");
        let cached_snapshot = self.etag_cache.lock().ok().and_then(|g| g.clone());

        let mut req = self.apply_auth_header(self.client.get(url));
        if let Some(ref cached) = cached_snapshot {
            req = req.header(IF_NONE_MATCH, &cached.etag);
        }

        let outcome = self.runtime.block_on(async move {
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;

            if resp.status() == StatusCode::NOT_MODIFIED {
                if let Some(cached) = cached_snapshot {
                    return Ok((Some(cached.etag), cached.state, true));
                }
                return Err(ApiError::Network(
                    "Received HTTP 304 Not Modified without local ETag cache".to_string(),
                ));
            }

            if !resp.status().is_success() {
                return Err(map_error_response(resp, false).await);
            }

            let etag = extract_etag(resp.headers());
            let wire: WireMarketState = resp
                .json()
                .await
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            Ok((etag, MarketStateResponse::from(wire), false))
        })?;

        let (etag_opt, state, was_not_modified) = outcome;
        if !was_not_modified {
            self.store_or_clear_etag(etag_opt, &state);
        }
        Ok(state)
    }

    fn submit_transaction(&self, tx: TransactionRequest) -> Result<MarketStateResponse, ApiError> {
        let tx_id = tx.tx_id.trim();
        let fill_type = tx.fill_type.trim();
        if tx_id.is_empty() || fill_type.is_empty() || tx.amount <= 0 {
            return Err(ApiError::Validation(
                "Invalid TransactionRequest: tx_id, fill_type, and amount > 0 are required"
                    .to_string(),
            ));
        }

        let url = self.endpoint("/api/v1/market/transactions");
        let payload = WireTransactionPayload {
            tx_id,
            fill_type,
            amount: tx.amount,
            tx_type: tx.tx_type.as_str(),
        };
        let req = self.apply_auth_header(self.client.post(url).json(&payload));
        let (etag_opt, state) = self.runtime.block_on(async move {
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;
            if !resp.status().is_success() {
                return Err(map_error_response(resp, true).await);
            }
            let etag = extract_etag(resp.headers());
            let wire: WireMarketState = resp
                .json()
                .await
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            Ok((etag, MarketStateResponse::from(wire)))
        })?;

        // Update or invalidate the ETag snapshot so a subsequent GET /market/state never returns pre-transaction data.
        self.store_or_clear_etag(etag_opt, &state);
        Ok(state)
    }

    fn fetch_contracts_state(
        &self,
        server_id: Option<&str>,
        server_secret: Option<&ServerSecret>,
    ) -> Result<ContractStateResponse, ApiError> {
        let clean_sid = server_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let url = self.endpoint("/api/v1/contracts/state");
        let cached_snapshot = self
            .contract_etag_cache
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .filter(|c| c.server_id == clean_sid);

        let mut req = match clean_sid {
            Some(ref sid) => self
                .apply_board_headers(self.client.get(url), sid, server_secret)
                .query(&[("server_id", sid.as_str())]),
            None => self.apply_auth_header(self.client.get(url)),
        };
        if let Some(ref cached) = cached_snapshot {
            req = req.header(IF_NONE_MATCH, &cached.etag);
        }

        let outcome = self.runtime.block_on(async move {
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;

            if resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::NOT_IMPLEMENTED
            {
                return Err(ApiError::NotSupported(format!("HTTP {}", resp.status())));
            }

            if resp.status() == StatusCode::NOT_MODIFIED {
                if let Some(cached) = cached_snapshot {
                    return Ok((Some(cached.etag), cached.state, true));
                }
                return Err(ApiError::Network(
                    "Received HTTP 304 Not Modified without local contracts ETag cache".to_string(),
                ));
            }

            if !resp.status().is_success() {
                return Err(map_error_response(resp, false).await);
            }

            let etag = extract_etag(resp.headers());
            let wire: WireContractState = resp
                .json()
                .await
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            Ok((etag, ContractStateResponse::from(wire), false))
        })?;

        let (etag_opt, state, was_not_modified) = outcome;
        if let Some(ref sid) = clean_sid {
            self.remember_savegame_identity(sid, state.server_secret.as_ref().or(server_secret));
        }
        if !was_not_modified {
            self.store_or_clear_contract_etag(clean_sid, etag_opt, &state);
        }
        Ok(state)
    }

    fn submit_contract_action(
        &self,
        action: ContractActionRequest,
        server_secret: Option<&ServerSecret>,
    ) -> Result<ContractStateResponse, ApiError> {
        let action_id = action.action_id.trim();
        let server_id = action.server_id.trim();
        if action_id.is_empty() || server_id.is_empty() {
            return Err(ApiError::Validation(
                "Invalid ContractActionRequest: action_id and server_id are required".to_string(),
            ));
        }

        let clean_fill = action
            .fill_type
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_uppercase());
        let payload = WireContractActionPayload {
            sub_action: action.sub_action.as_str(),
            action_id,
            server_id,
            farm_id: action.farm_id.filter(|&id| id > 0),
            farm_name: action
                .farm_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            remote_id: action
                .remote_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            fill_type: clean_fill.as_deref(),
            liters: action.liters.filter(|&l| l > 0),
            base_trend_factor: action
                .base_trend_factor
                .filter(|f| f.is_finite() && *f > 0.0)
                .map(|f| ((f as f64) * 10_000.0).round() / 10_000.0),
            bonus_pct: action.bonus_pct,
            price_delta_pct: action.price_delta_pct,
            allow_partial: action.allow_partial,
            duration_hours: action.duration_hours.filter(|&h| h > 0),
            was_bale: action.was_bale,
            was_pallet: action.was_pallet,
            settlement_ids: action
                .settlement_ids
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        };

        let url = self.endpoint("/api/v1/contracts/actions");
        let req =
            self.apply_board_headers(self.client.post(url).json(&payload), server_id, server_secret);
        let (etag_opt, raw_json) = self.runtime.block_on(async move {
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;
            if resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::NOT_IMPLEMENTED
            {
                return Err(ApiError::NotSupported(format!("HTTP {}", resp.status())));
            }
            if !resp.status().is_success() {
                return Err(map_error_response(resp, true).await);
            }
            let etag = extract_etag(resp.headers());
            let json_val: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            Ok((etag, json_val))
        })?;

        let has_state_arrays = raw_json.get("contracts").is_some()
            || raw_json.get("direct_offers").is_some()
            || raw_json.get("directOffers").is_some()
            || raw_json.get("action_results").is_some()
            || raw_json.get("actionResults").is_some()
            || raw_json.get("settlements").is_some()
            || raw_json.get("state").is_some();

        if has_state_arrays {
            let state_val = raw_json.get("state").cloned().unwrap_or_else(|| raw_json.clone());
            let wire_state: WireContractState = serde_json::from_value(state_val)
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            let mut parsed_state = ContractStateResponse::from(wire_state);
            let issued_secret = parsed_state
                .server_secret
                .take()
                .or_else(|| issued_secret_from_json(&raw_json));

            if let Some(res_val) = raw_json.get("result") {
                if let Ok(wire_res) = serde_json::from_value::<WireContractActionResult>(res_val.clone())
                {
                    let dto = ContractActionResultDto::from(wire_res);
                    if !dto.action_id.is_empty()
                        && !parsed_state
                            .action_results
                            .iter()
                            .any(|r| r.action_id == dto.action_id)
                    {
                        parsed_state.action_results.push(dto);
                    }
                }
            }

            // Cached without the secret: a later 304 must not look like a newly issued one.
            self.store_or_clear_contract_etag(Some(server_id.to_string()), etag_opt, &parsed_state);
            self.remember_savegame_identity(server_id, issued_secret.as_ref().or(server_secret));
            parsed_state.server_secret = issued_secret;
            return Ok(parsed_state);
        }

        // Fallback resilience if a backend returns a single ActionResult object directly instead of ContractStateResponse:
        // invalidate contract ETag cache, fetch fresh board state, and fold the immediate ActionResult into it.
        let single_res: Option<ContractActionResultDto> = raw_json
            .get("result")
            .cloned()
            .or_else(|| raw_json.get("action_id").is_some().then(|| raw_json.clone()))
            .and_then(|v| serde_json::from_value::<WireContractActionResult>(v).ok())
            .map(ContractActionResultDto::from)
            .filter(|r| !r.action_id.is_empty());

        let prev_cached = self
            .contract_etag_cache
            .lock()
            .ok()
            .and_then(|mut g| g.take())
            .map(|c| c.state);

        let mut board_state = self
            .fetch_contracts_state(Some(server_id), server_secret)
            .or_else(|err| prev_cached.ok_or(err))?;

        if let Some(dto) = single_res {
            if !board_state
                .action_results
                .iter()
                .any(|r| r.action_id == dto.action_id)
            {
                board_state.action_results.push(dto);
            }
        }
        board_state.server_secret = issued_secret_from_json(&raw_json);
        self.remember_savegame_identity(
            server_id,
            board_state.server_secret.as_ref().or(server_secret),
        );
        Ok(board_state)
    }

    fn fetch_vehicles_state(
        &self,
        server_id: Option<&str>,
        server_secret: Option<&ServerSecret>,
    ) -> Result<VehicleStateResponse, ApiError> {
        let clean_sid = server_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let url = self.endpoint("/api/v1/vehicles/state");
        let cached_snapshot = self
            .vehicle_etag_cache
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .filter(|c| c.server_id == clean_sid);

        let mut req = match clean_sid {
            Some(ref sid) => self
                .apply_board_headers(self.client.get(url), sid, server_secret)
                .query(&[("server_id", sid.as_str())]),
            None => self.apply_auth_header(self.client.get(url)),
        };
        if let Some(ref cached) = cached_snapshot {
            req = req.header(IF_NONE_MATCH, &cached.etag);
        }

        let (etag_opt, state, was_not_modified) = self.runtime.block_on(async move {
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;
            if resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::NOT_IMPLEMENTED
            {
                return Err(ApiError::NotSupported(format!("HTTP {}", resp.status())));
            }
            if resp.status() == StatusCode::NOT_MODIFIED {
                if let Some(cached) = cached_snapshot {
                    return Ok((Some(cached.etag), cached.state, true));
                }
                return Err(ApiError::Network(
                    "Received HTTP 304 Not Modified without local vehicles ETag cache".to_string(),
                ));
            }
            if !resp.status().is_success() {
                return Err(map_error_response(resp, false).await);
            }
            let etag = extract_etag(resp.headers());
            let wire: WireVehicleState = resp
                .json()
                .await
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            Ok((etag, VehicleStateResponse::from(wire), false))
        })?;

        if let Some(ref sid) = clean_sid {
            self.remember_savegame_identity(sid, state.server_secret.as_ref().or(server_secret));
        }
        if !was_not_modified {
            self.store_or_clear_vehicle_etag(clean_sid, etag_opt, &state);
        }
        Ok(state)
    }

    fn submit_vehicle_action(
        &self,
        action: VehicleActionRequest,
        server_secret: Option<&ServerSecret>,
    ) -> Result<VehicleStateResponse, ApiError> {
        let action_id = action.action_id.trim();
        let server_id = action.server_id.trim();
        if action_id.is_empty() || server_id.is_empty() {
            return Err(ApiError::Validation(
                "Invalid VehicleActionRequest: action_id and server_id are required".to_string(),
            ));
        }
        let non_empty = |v: &Option<String>| -> Option<String> {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        };
        let (farm_name, remote_id, create_action_id, sale_mode, settlement_ids) = (
            non_empty(&action.farm_name),
            non_empty(&action.remote_id),
            non_empty(&action.create_action_id),
            non_empty(&action.sale_mode).map(|s| s.to_ascii_uppercase()),
            non_empty(&action.settlement_ids),
        );
        let payload = WireVehicleActionPayload {
            sub_action: action.sub_action.as_str(),
            action_id,
            server_id,
            farm_id: action.farm_id.filter(|&id| id > 0),
            farm_name: farm_name.as_deref(),
            remote_id: remote_id.as_deref(),
            create_action_id: create_action_id.as_deref(),
            sale_mode: sale_mode.as_deref(),
            price_factor: action
                .price_factor
                .filter(|f| f.is_finite())
                .map(|f| (f * 10_000.0).round() / 10_000.0),
            duration_hours: action.duration_hours,
            settlement_ids: settlement_ids.as_deref(),
            vehicle: action.vehicle.as_ref().map(WireVehicle::from),
        };

        let url = self.endpoint("/api/v1/vehicles/actions");
        let req =
            self.apply_board_headers(self.client.post(url).json(&payload), server_id, server_secret);
        let (etag_opt, state) = self.runtime.block_on(async move {
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;
            if resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::NOT_IMPLEMENTED
            {
                return Err(ApiError::NotSupported(format!("HTTP {}", resp.status())));
            }
            if !resp.status().is_success() {
                return Err(map_error_response(resp, true).await);
            }
            let etag = extract_etag(resp.headers());
            let wire: WireVehicleState = resp
                .json()
                .await
                .map_err(|e| ApiError::Validation(e.to_string()))?;
            Ok((etag, VehicleStateResponse::from(wire)))
        })?;

        // Cached without the secret: a later 304 must not look like a newly issued one.
        self.store_or_clear_vehicle_etag(Some(server_id.to_string()), etag_opt, &state);
        self.remember_savegame_identity(server_id, state.server_secret.as_ref().or(server_secret));
        Ok(state)
    }

    fn set_loaded_map(&self, map: Option<LoadedMap>) {
        if let Ok(mut guard) = self.loaded_map.write() {
            *guard = map;
        }
    }

    fn forget_savegame_identity(&self) {
        self.clear_savegame_identity();
    }

    fn fetch_logo_bytes(&self, logo_url: &str) -> Result<Vec<u8>, ApiError> {
        let base_url = self
            .base_url
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| crate::config::DEFAULT_OFFICIAL_BACKEND_URL.to_string());
        let has_token = self
            .market_token
            .read()
            .ok()
            .and_then(|g| g.clone())
            .is_some();

        let Some(safe_url) = resolve_safe_logo_url(Some(logo_url.to_string()), &base_url, has_token)
        else {
            return Err(ApiError::Validation(
                "Untrusted or unauthorized market logo URL.".to_string(),
            ));
        };

        let same_authority_as_api =
            extract_scheme_and_authority(&safe_url) == extract_scheme_and_authority(&base_url);
        let mut req = self.client.get(&safe_url);
        if same_authority_as_api {
            req = self.apply_auth_header(req);
        }

        self.runtime.block_on(async move {
            let mut resp = req
                .send()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?;
            if !resp.status().is_success() {
                return Err(map_error_response(resp, false).await);
            }
            if let Some(len) = resp.content_length() {
                if len > MAX_LOGO_RAW_BYTES as u64 {
                    return Err(ApiError::Validation(
                        "Remote logo exceeds 8 MB size limit.".to_string(),
                    ));
                }
            }

            let mut buf = Vec::new();
            while let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|e| ApiError::Network(format_reqwest_error(&e)))?
            {
                if buf.len().saturating_add(chunk.len()) > MAX_LOGO_RAW_BYTES {
                    return Err(ApiError::Validation(
                        "Remote logo exceeds 8 MB size limit.".to_string(),
                    ));
                }
                buf.extend_from_slice(&chunk);
            }

            if buf.is_empty() {
                return Err(ApiError::Validation(
                    "Remote logo response body was empty.".to_string(),
                ));
            }
            Ok(buf)
        })
    }
}

/// Hybrid 3-tier branding resolver supporting both explicit backend fields (`branding_tier` / `is_private_market`)
/// and automatic derivation from `(has_token, market_name, market_logo_url)`.
pub fn derive_handshake_branding(
    has_token: bool,
    raw_market_name: &str,
    resolved_logo_url: Option<String>,
    explicit_tier: Option<u8>,
    explicit_private: Option<bool>,
    explicit_has_logo: Option<bool>,
) -> (u8, bool, String, Option<String>) {
    if !has_token {
        return (0, false, "Global Market".to_string(), None);
    }

    let logo_url = if explicit_has_logo == Some(false) {
        None
    } else {
        resolved_logo_url
    };

    let clean_name = sanitize_market_name(raw_market_name);
    let is_official_name = clean_name.as_deref().map_or(true, is_official_market_name);

    match explicit_tier {
        Some(0) => (0, false, "Global Market".to_string(), None),
        Some(1) => {
            if logo_url.is_some() {
                (1, false, "Global Market".to_string(), logo_url)
            } else {
                (0, false, "Global Market".to_string(), None)
            }
        }
        Some(2) => (
            2,
            true,
            clean_name.unwrap_or_else(|| "Community Market".to_string()),
            None,
        ),
        Some(3) => {
            let priv_name = clean_name.unwrap_or_else(|| "Community Market".to_string());
            if logo_url.is_some() {
                (3, true, priv_name, logo_url)
            } else {
                (2, true, priv_name, None)
            }
        }
        _ => {
            let is_priv = explicit_private.unwrap_or(!is_official_name);
            match (is_priv, logo_url.is_some()) {
                (false, false) => (0, false, "Global Market".to_string(), None),
                (false, true) => (1, false, "Global Market".to_string(), logo_url),
                (true, false) => (
                    2,
                    true,
                    clean_name.unwrap_or_else(|| "Community Market".to_string()),
                    None,
                ),
                (true, true) => (
                    3,
                    true,
                    clean_name.unwrap_or_else(|| "Community Market".to_string()),
                    logo_url,
                ),
            }
        }
    }
}

pub fn is_official_market_name(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("Global Market")
        || trimmed.eq_ignore_ascii_case("Global Market Official")
        || trimmed.eq_ignore_ascii_case("Official Global Market")
}

fn normalize_base_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn clean_token(token: Option<String>) -> Option<String> {
    token.and_then(|t| {
        let trimmed = t.trim().to_string();
        if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn clean_client_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 64
        || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn clean_version_opt(raw: Option<String>) -> Option<String> {
    raw.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty()
            || trimmed.len() > 32
            || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Classifies a `503` body with code `MAINTENANCE` (Spec Section 3.1.3), either as
/// `{"detail": {"code": "MAINTENANCE", "notice_id": "...", "until": 1764624600}}` or as plain
/// `{"detail": "MAINTENANCE"}`. Without `until`, the end is derived from the `Retry-After` header.
pub fn parse_maintenance_error(
    root: &serde_json::Value,
    retry_after_hdr: Option<u64>,
) -> Option<ApiError> {
    let from_retry_after = || {
        retry_after_hdr.map(|secs| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                + secs.min(7 * 86_400)
        })
    };
    for candidate in [root.get("detail"), Some(root)].into_iter().flatten() {
        match candidate {
            serde_json::Value::Object(obj) => {
                let code = obj.get("code").and_then(|v| v.as_str()).unwrap_or("");
                if code.trim().eq_ignore_ascii_case("MAINTENANCE") {
                    let notice_id = obj
                        .get("notice_id")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| s.chars().filter(|c| !c.is_control()).take(64).collect());
                    let until = obj
                        .get("until")
                        .and_then(|v| v.as_u64())
                        .filter(|secs| *secs > 0)
                        .or_else(from_retry_after);
                    return Some(ApiError::Maintenance { notice_id, until });
                }
            }
            serde_json::Value::String(s) if s.trim().eq_ignore_ascii_case("MAINTENANCE") => {
                return Some(ApiError::Maintenance {
                    notice_id: None,
                    until: from_retry_after(),
                });
            }
            _ => {}
        }
    }
    None
}

/// Classifies structured 403/429 key-slot & client-binding error codes from the FastAPI backend:
/// - `TOKEN_SLOT_LIMIT_REACHED` (`max_slots`) -> [`ApiError::SlotLimitReached`]
/// - `CLIENT_IP_LOCKED` (`retry_after` or `Retry-After` header) -> [`ApiError::ClientIpLocked`]
/// - `CLIENT_ID_REQUIRED` -> [`ApiError::ClientIdRequired`]
/// - `SERVER_SECRET_INVALID` -> [`ApiError::ServerSecretInvalid`] (object form)
/// - `DEV_TOKEN_REQUIRED` / `DEV_TOKEN_INVALID` -> [`ApiError::DevTokenRejected`] (object form, development servers)
/// - `CLIENT_BLOCKED` -> [`ApiError::ClientBlocked`] (object form)
/// - `CONTENT_NOT_SUPPORTED` (`label`) -> [`ApiError::ContentNotSupported`] (object form)
fn parse_classified_auth_error(
    root: &serde_json::Value,
    retry_after_hdr: Option<u64>,
) -> Option<ApiError> {
    for candidate in [root.get("detail"), Some(root)].into_iter().flatten() {
        match candidate {
            serde_json::Value::Object(obj) => {
                let code = obj
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if code.eq_ignore_ascii_case("TOKEN_SLOT_LIMIT_REACHED")
                    || code.eq_ignore_ascii_case("SLOT_LIMIT_REACHED")
                {
                    let slots = obj
                        .get("max_slots")
                        .or_else(|| obj.get("limit"))
                        .and_then(|v| v.as_u64())
                        .map(|n| n.clamp(1, 10_000) as u32)
                        .unwrap_or(16);
                    return Some(ApiError::SlotLimitReached { max_slots: slots });
                }
                if code.eq_ignore_ascii_case("CLIENT_IP_LOCKED") {
                    let secs = obj
                        .get("retry_after")
                        .and_then(|v| v.as_u64())
                        .or(retry_after_hdr)
                        .unwrap_or(3600)
                        .clamp(1, 86_400);
                    return Some(ApiError::ClientIpLocked {
                        retry_after_sec: secs,
                    });
                }
                if code.eq_ignore_ascii_case("CLIENT_ID_REQUIRED") {
                    return Some(ApiError::ClientIdRequired);
                }
                if code.eq_ignore_ascii_case("SERVER_SECRET_INVALID") {
                    return Some(ApiError::ServerSecretInvalid);
                }
                if code.eq_ignore_ascii_case("DEV_TOKEN_REQUIRED") {
                    return Some(ApiError::DevTokenRejected { missing: true });
                }
                if code.eq_ignore_ascii_case("DEV_TOKEN_INVALID") {
                    return Some(ApiError::DevTokenRejected { missing: false });
                }
                if code.eq_ignore_ascii_case("CLIENT_BLOCKED") {
                    return Some(ApiError::ClientBlocked);
                }
                if code.eq_ignore_ascii_case("CONTENT_NOT_SUPPORTED") {
                    let label = obj
                        .get("label")
                        .and_then(|v| v.as_str())
                        .and_then(clean_content_label);
                    return Some(ApiError::ContentNotSupported { label });
                }
            }
            serde_json::Value::String(s) => {
                let trimmed = s.trim();
                let upper = trimmed.to_ascii_uppercase();
                if let Some(rest) = upper
                    .strip_prefix("TOKEN_SLOT_LIMIT_REACHED")
                    .or_else(|| upper.strip_prefix("SLOT_LIMIT_REACHED"))
                {
                    let parsed_num = rest
                        .trim_start_matches([':', '=', ' '])
                        .parse::<u32>()
                        .ok()
                        .map(|n| n.clamp(1, 10_000))
                        .unwrap_or(16);
                    return Some(ApiError::SlotLimitReached {
                        max_slots: parsed_num,
                    });
                }
                if let Some(rest) = upper.strip_prefix("CLIENT_IP_LOCKED") {
                    let parsed_secs = rest
                        .trim_start_matches([':', '=', ' '])
                        .parse::<u64>()
                        .ok()
                        .or(retry_after_hdr)
                        .unwrap_or(3600)
                        .clamp(1, 86_400);
                    return Some(ApiError::ClientIpLocked {
                        retry_after_sec: parsed_secs,
                    });
                }
                if upper == "CLIENT_ID_REQUIRED" {
                    return Some(ApiError::ClientIdRequired);
                }
            }
            _ => {}
        }
    }
    None
}

/// Longest `label` of a `CONTENT_NOT_SUPPORTED` answer that is shown.
const MAX_CONTENT_LABEL_CHARS: usize = 64;

/// Name of the refused content for display: trimmed, without control characters, at most
/// [`MAX_CONTENT_LABEL_CHARS`] characters.
fn clean_content_label(raw: &str) -> Option<String> {
    let clean: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_CONTENT_LABEL_CHARS)
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

/// Validates and resolves an optional Community Market logo URL.
/// Custom logos require an active `X-Market-Token` (`has_token == true`) and are strictly
/// pinned to official GlobalMarket HTTPS origins (`img.gm.bydora.de`, `gm.bydora.de`,
/// `dev.gm.bydora.de`, or the active HTTPS `base_url`) plus local loopback (`127.0.0.1` / `localhost`)
/// so external third-party tracking domains can never be injected into the client WebView.
pub fn resolve_safe_logo_url(
    raw_logo: Option<String>,
    base_url: &str,
    has_token: bool,
) -> Option<String> {
    if !has_token {
        return None;
    }
    let trimmed = raw_logo?.trim().to_string();
    if trimmed.is_empty()
        || trimmed.len() > 2048
        || trimmed.contains('@')
        || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }

    let lower_raw = trimmed.to_ascii_lowercase();
    let resolved = if lower_raw.starts_with("https://") || lower_raw.starts_with("http://") {
        trimmed
    } else if trimmed.starts_with('/') {
        format!("{}{}", base_url.trim_end_matches('/'), trimmed)
    } else {
        format!(
            "{}/{}",
            crate::config::OFFICIAL_LOGO_CDN_URL.trim_end_matches('/'),
            trimmed
        )
    };

    is_trusted_logo_origin(&resolved, base_url).then_some(resolved)
}

fn is_trusted_logo_origin(url: &str, base_url: &str) -> bool {
    if url.contains('@') {
        return false;
    }
    if let Some((scheme, host_port)) = extract_scheme_and_authority(url) {
        let host = strip_port(&host_port);
        match scheme.as_str() {
            "https" => {
                if matches!(
                    host,
                    "img.gm.bydora.de" | "gm.bydora.de" | "dev.gm.bydora.de"
                ) && (host_port == host || host_port == format!("{host}:443"))
                {
                    return true;
                }
                if let Some((base_scheme, base_auth)) = extract_scheme_and_authority(base_url) {
                    if base_scheme == "https" && host_port == base_auth {
                        return true;
                    }
                }
                false
            }
            "http" => matches!(host, "127.0.0.1" | "localhost"),
            _ => false,
        }
    } else {
        false
    }
}

/// The developer token goes only to the official development server (HTTPS) and to a local
/// loopback backend, never to production or to any other custom URL.
pub fn dev_token_allowed(base_url: &str) -> bool {
    let Some((scheme, authority)) = extract_scheme_and_authority(base_url) else {
        return false;
    };
    let dev_authority = extract_scheme_and_authority(crate::config::OFFICIAL_DEV_BACKEND_URL)
        .map(|(_, a)| a)
        .unwrap_or_default();
    let host = strip_port(&authority);
    match scheme.as_str() {
        "https" => authority == dev_authority || matches!(host, "127.0.0.1" | "localhost"),
        "http" => matches!(host, "127.0.0.1" | "localhost"),
        _ => false,
    }
}

fn extract_scheme_and_authority(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim();
    let (scheme_raw, rest) = trimmed.split_once("://")?;
    let scheme = scheme_raw.to_ascii_lowercase();
    let authority = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some((scheme, authority))
}

fn strip_port(authority: &str) -> &str {
    authority
        .rsplit_once(':')
        .map(|(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                h
            } else {
                authority
            }
        })
        .unwrap_or(authority)
}
