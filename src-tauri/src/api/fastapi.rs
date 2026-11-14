//! Production HTTP client for the Python / FastAPI backend (`FS25: Global Market v2.0.0`).
//!
//! Aligned 1:1 with `docs/openapi.json`:
//! - `GET  /api/v1/meta/status`         -> `MetaStatus`
//! - `GET  /api/v1/market/state`        -> `200 MarketState` or `304 Not Modified` via `ETag` / `If-None-Match`
//! - `POST /api/v1/market/whitelist`    -> `WhitelistRequest` -> `200 MarketState`
//! - `POST /api/v1/market/transactions` -> `TransactionRequest` -> `200 MarketState`
//! - Optional `X-Market-Token` header for private/community/logo keys (absent = official public market).
//! - Persistent random `X-Client-Id` header so the backend can enforce 16/32 active client slots per key without hardware tracking.

use crate::api::types::{
    FillTypeStatus, MarketItemDto, MarketStateResponse, ServerHandshake, TransactionRequest,
    WhitelistSyncRequest,
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

pub struct FastApiBackend {
    base_url: RwLock<String>,
    market_token: RwLock<Option<String>>,
    client_id: RwLock<Option<String>>,
    etag_cache: Mutex<Option<CachedMarketState>>,
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
            etag_cache: Mutex::new(None),
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

    fn apply_auth_header(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(cid) = self.client_id.read().ok().and_then(|g| g.clone()) {
            req = req.header("X-Client-Id", cid);
        }
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

/// Classifies structured 403/429 key-slot & client-binding error codes from the FastAPI backend:
/// - `TOKEN_SLOT_LIMIT_REACHED` (`max_slots`) -> [`ApiError::SlotLimitReached`]
/// - `CLIENT_IP_LOCKED` (`retry_after` or `Retry-After` header) -> [`ApiError::ClientIpLocked`]
/// - `CLIENT_ID_REQUIRED` -> [`ApiError::ClientIdRequired`]
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
