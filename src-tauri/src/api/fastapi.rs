//! Production HTTP client for the Python / FastAPI backend (`FS25: Global Market v2.0.0`).
//!
//! Aligned 1:1 with `docs/openapi.json`:
//! - `GET  /api/v1/meta/status`         -> `MetaStatus`
//! - `GET  /api/v1/market/state`        -> `200 MarketState` or `304 Not Modified` via `ETag` / `If-None-Match`
//! - `POST /api/v1/market/whitelist`    -> `WhitelistRequest` -> `200 MarketState`
//! - `POST /api/v1/market/transactions` -> `TransactionRequest` -> `200 MarketState`
//! - Optional `X-Market-Token` header for private/community markets (absent = official public market).

use crate::api::types::{
    FillTypeStatus, MarketItemDto, MarketStateResponse, ServerHandshake, TransactionRequest,
    WhitelistSyncRequest,
};
use crate::api::{ApiError, BackendService};
use reqwest::header::{ETAG, IF_NONE_MATCH};
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

pub struct FastApiBackend {
    base_url: RwLock<String>,
    market_token: RwLock<Option<String>>,
    etag_cache: Mutex<Option<CachedMarketState>>,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}

impl FastApiBackend {
    pub fn new(base_url: String, market_token: Option<String>) -> Result<Self, String> {
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
            etag_cache: Mutex::new(None),
            client,
            runtime,
        })
    }

    /// Updates target base URL and optional `X-Market-Token`, clearing any cached ETag snapshot
    /// whenever the target endpoint or market scope changes.
    pub fn update_connection_config(&self, base_url: String, market_token: Option<String>) {
        let normalized_url = normalize_base_url(&base_url);
        let cleaned_token = clean_token(market_token);

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

        if changed {
            self.clear_etag_cache();
        }
    }

    pub fn clear_etag_cache(&self) {
        if let Ok(mut guard) = self.etag_cache.lock() {
            *guard = None;
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

    fn endpoint(&self, suffix: &str) -> String {
        let base = self
            .base_url
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| crate::config::DEFAULT_OFFICIAL_BACKEND_URL.to_string());
        format!("{base}{suffix}")
    }

    fn apply_auth_header(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let token = self.market_token.read().ok().and_then(|g| g.clone());
        match token {
            Some(t) => req.header("X-Market-Token", t),
            None => req,
        }
    }
}

#[derive(Debug, Deserialize)]
struct WireMetaStatus {
    #[serde(default = "default_true")]
    online: bool,
    #[serde(default = "default_min_ver")]
    min_exe_version: String,
    #[serde(default = "default_poll")]
    poll_interval_sec: u64,
    #[serde(default = "default_market_name")]
    market_name: String,
    #[serde(default = "default_api_ver")]
    api_version: String,
    #[serde(default)]
    market_logo_url: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_min_ver() -> String {
    "1.0.0".to_string()
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
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return ApiError::Unauthorized;
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
        self.runtime.block_on(async move {
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
            let market_logo_url =
                resolve_safe_logo_url(parsed.market_logo_url, &base_url, has_token);
            Ok(ServerHandshake {
                online: parsed.online,
                min_exe_version: parsed.min_exe_version,
                poll_interval_sec: parsed.poll_interval_sec.max(2),
                market_name: parsed.market_name,
                api_version: parsed.api_version,
                market_logo_url,
            })
        })
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
}

fn normalize_base_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn clean_token(token: Option<String>) -> Option<String> {
    token.and_then(|t| {
        let trimmed = t.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
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
