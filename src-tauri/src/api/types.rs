//! Shared domain & DTO models between the XML bridge, UI, Mock backend, and FastAPI client.

use serde::{Deserialize, Serialize};

/// Moderation status of a FillType in the Global Market backend (Interface Spec §3.2 & §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FillTypeStatus {
    Approved,
    Pending,
    Rejected,
}

impl FillTypeStatus {
    pub fn as_xml_str(self) -> &'static str {
        match self {
            Self::Approved => "APPROVED",
            Self::Pending => "PENDING",
            Self::Rejected => "REJECTED",
        }
    }

    pub fn from_wire_str(raw: &str) -> Self {
        match raw.trim().to_ascii_uppercase().as_str() {
            "PENDING" | "NEED_REVIEW" => Self::Pending,
            "REJECTED" => Self::Rejected,
            _ => Self::Approved,
        }
    }
}

impl Default for FillTypeStatus {
    fn default() -> Self {
        Self::Approved
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TransactionType {
    Buy,
    Sell,
}

impl TransactionType {
    pub fn from_xml_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "BUY" => Some(Self::Buy),
            "SELL" => Some(Self::Sell),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhitelistItemDto {
    pub name: String,
    pub title: String,
    pub category: String,
    pub item_type: String,
    pub base_price: f32,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhitelistSyncRequest {
    pub lang: String,
    pub items: Vec<WhitelistItemDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketItemDto {
    pub fill_type: String,
    pub title: Option<String>,
    pub fill_level: i64,
    pub trend: f32,
    pub great_demand: bool,
    pub great_demand_bonus: f32,
    pub great_demand_remaining: i64,
    pub status: FillTypeStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketStateResponse {
    pub updated_at: i64,
    pub items: Vec<MarketItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionRequest {
    pub tx_id: String,
    pub fill_type: String,
    /// Traded volume in whole liters (`> 0`).
    pub amount: i64,
    pub tx_type: TransactionType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerHandshake {
    pub online: bool,
    pub min_exe_version: String,
    /// Optional recommended ("nice-to-have") client version announced by the backend without blocking online trading.
    #[serde(default)]
    pub latest_exe_version: Option<String>,
    pub poll_interval_sec: u64,
    pub market_name: String,
    pub api_version: String,
    /// Online market branding tier (`0` = Standard, `1` = Logo on Official Market, `2` = Private Market, `3` = Private + Logo).
    #[serde(default)]
    pub branding_tier: u8,
    /// `true` when operating on an isolated private/community market (`branding_tier` `2` or `3`).
    #[serde(default)]
    pub is_private_market: bool,
    /// Optional custom logo URL returned by the backend when a valid Community/Logo `X-Market-Token` is active.
    #[serde(default)]
    pub market_logo_url: Option<String>,
    /// Capability gate for Stage 2 cross-server Supply Contracts & Direct Offers (`#25`).
    #[serde(default)]
    pub contracts_supported: bool,
    /// Capability gate for the Stage 3 cross-server used vehicle market (Spec Section 3.6).
    #[serde(default)]
    pub vehicles_supported: bool,
    /// Optional maintenance / restart / info notice (Spec Section 3.1.3).
    #[serde(default)]
    pub notice: Option<ServerNotice>,
    /// `production` or `development` (`server_env`, API 2.10); `None` for older backends.
    #[serde(default)]
    pub server_env: Option<String>,
}

/// Normalizes `server_env` from `/meta/status`: only the two documented values are kept.
pub fn normalize_server_env(raw: Option<&str>) -> Option<String> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "development" | "dev" => Some("development".to_string()),
        "production" | "prod" => Some("production".to_string()),
        _ => None,
    }
}

/// Assumed duration of a `restart` notice without `ends_at`.
pub const NOTICE_RESTART_DEFAULT_SECS: u64 = 600;
/// Assumed duration of a `maintenance` notice without `ends_at`, used only to expire it.
pub const NOTICE_MAINTENANCE_FALLBACK_SECS: u64 = 6 * 3600;
/// A downtime notice stays relevant this long after its (expected) end.
pub const NOTICE_EXPIRY_GRACE_SECS: u64 = 1800;
/// Outages this long before `starts_at` already count as the announced downtime.
pub const NOTICE_WINDOW_LEAD_SECS: u64 = 300;
const NOTICE_MAX_ID_CHARS: usize = 64;
const NOTICE_MAX_MESSAGE_CHARS: usize = 500;
const NOTICE_MAX_MESSAGES: usize = 32;

/// Server notice from `GET /api/v1/meta/status` (`notice`, Spec Section 3.1.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerNotice {
    /// Stable id; client and mod announce each id only once.
    pub id: String,
    /// `maintenance`, `restart` or `info` (unknown backend types are stored as `info`).
    pub notice_type: String,
    /// Start as UTC unix seconds (always set for `maintenance` / `restart`).
    #[serde(default)]
    pub starts_at: Option<u64>,
    /// Expected end as UTC unix seconds, as sent by the backend.
    #[serde(default)]
    pub ends_at: Option<u64>,
    /// Optional short extra text per language code (`de`, `en`, ...).
    #[serde(default)]
    pub message: std::collections::BTreeMap<String, String>,
}

impl ServerNotice {
    /// Parses the backend `notice` object leniently. Returns `None` for missing or invalid notices, so a
    /// malformed notice never breaks the status handshake.
    pub fn from_wire(value: &serde_json::Value) -> Option<Self> {
        let obj = value.as_object()?;
        let id: String = obj
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?
            .chars()
            .filter(|c| !c.is_control())
            .take(NOTICE_MAX_ID_CHARS)
            .collect();
        let starts_at = obj.get("starts_at").and_then(wire_unix_secs);
        let ends_at = obj
            .get("ends_at")
            .and_then(wire_unix_secs)
            .filter(|end| starts_at.map_or(true, |start| *end >= start));
        let raw_type = obj
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        // Downtime notices without a start are useless for planning, so they degrade to plain info.
        let notice_type = match raw_type.as_str() {
            "maintenance" | "restart" if starts_at.is_some() => raw_type,
            _ => "info".to_string(),
        };
        let message = obj
            .get("message")
            .and_then(|v| v.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(lang, text)| {
                        let lang = lang.trim().to_ascii_lowercase();
                        let text: String = text
                            .as_str()?
                            .trim()
                            .chars()
                            .take(NOTICE_MAX_MESSAGE_CHARS)
                            .collect();
                        (!lang.is_empty() && !text.is_empty()).then_some((lang, text))
                    })
                    .take(NOTICE_MAX_MESSAGES)
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            id,
            notice_type,
            starts_at,
            ends_at,
            message,
        })
    }

    /// `true` for `maintenance` and `restart`, which announce a downtime.
    pub fn is_downtime(&self) -> bool {
        matches!(self.notice_type.as_str(), "maintenance" | "restart")
    }

    /// End shown to players: `ends_at`, or `starts_at + 600` for a `restart` without an end.
    pub fn display_end(&self) -> Option<u64> {
        self.ends_at.or_else(|| match self.notice_type.as_str() {
            "restart" => self.starts_at.map(|s| s + NOTICE_RESTART_DEFAULT_SECS),
            _ => None,
        })
    }

    /// End used for expiry; an open-ended `maintenance` expires after a fallback duration.
    fn expiry_end(&self) -> Option<u64> {
        self.display_end().or_else(|| match self.notice_type.as_str() {
            "maintenance" => self.starts_at.map(|s| s + NOTICE_MAINTENANCE_FALLBACK_SECS),
            _ => None,
        })
    }

    /// `true` once the notice is no longer relevant (end plus grace period passed).
    pub fn is_expired(&self, now_unix: u64) -> bool {
        self.expiry_end()
            .is_some_and(|end| now_unix > end + NOTICE_EXPIRY_GRACE_SECS)
    }

    /// `true` while an outage at `now_unix` falls into this announced downtime window.
    pub fn covers_outage_at(&self, now_unix: u64) -> bool {
        self.is_downtime()
            && self
                .starts_at
                .is_some_and(|start| now_unix + NOTICE_WINDOW_LEAD_SECS >= start)
            && !self.is_expired(now_unix)
    }

    /// Extra text in the first available language of `langs`, then English, then any language.
    pub fn message_for(&self, langs: &[&str]) -> Option<String> {
        langs
            .iter()
            .map(|l| l.trim().to_ascii_lowercase())
            .chain(std::iter::once("en".to_string()))
            .find_map(|l| self.message.get(&l).cloned())
            .or_else(|| self.message.values().next().cloned())
    }
}

fn wire_unix_secs(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
        .filter(|secs| *secs > 0)
}

/// Maximum accepted length of a savegame secret (same limit as the mod).
pub const MAX_SERVER_SECRET_LEN: usize = 256;

/// Loaded map reported by the mod in `modInfo.xml` (Spec Section 3.1.1), sent as the optional
/// `X-Map-Id` / `X-Map-Mod` / `X-Map-Hash` headers so the backend can refuse unsupported content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedMap {
    /// `gmModInfo#mapId`.
    pub map_id: String,
    /// `gmModInfo#mapModName`, `None` for basegame and DLC maps.
    pub map_mod_name: Option<String>,
    /// `gmModInfo#mapHash`, `None` for basegame and DLC maps (`dev` for an unpacked map folder).
    pub map_hash: Option<String>,
}

/// Longest accepted `modInfo.xml` map value; longer ones are dropped instead of cut.
pub const MAX_MAP_VALUE_LEN: usize = 128;

impl LoadedMap {
    /// Builds the map from the raw attributes. `None` without a usable `mapId` (older mod versions).
    pub fn from_attributes(map_id: &str, map_mod_name: &str, map_hash: &str) -> Option<Self> {
        Some(Self {
            map_id: header_safe_map_value(map_id)?,
            map_mod_name: header_safe_map_value(map_mod_name),
            map_hash: header_safe_map_value(map_hash),
        })
    }
}

/// Trimmed value that fits into an HTTP header: printable ASCII and spaces, at most
/// [`MAX_MAP_VALUE_LEN`] characters. Anything else counts as missing.
fn header_safe_map_value(raw: &str) -> Option<String> {
    let clean = raw.trim();
    let valid = !clean.is_empty()
        && clean.len() <= MAX_MAP_VALUE_LEN
        && clean.bytes().all(|b| b == b' ' || b.is_ascii_graphic());
    valid.then(|| clean.to_string())
}

/// Credential of a savegame on the shared online pool (`X-Server-Secret`, Spec Section 3.5).
/// Never logged: `Debug` is redacted and the type is not serializable.
#[derive(Clone, PartialEq, Eq)]
pub struct ServerSecret(String);

impl ServerSecret {
    /// Accepts opaque tokens without markup or whitespace only; anything else counts as no secret.
    pub fn parse(raw: &str) -> Option<Self> {
        let clean = raw.trim();
        let valid = !clean.is_empty()
            && clean.len() <= MAX_SERVER_SECRET_LEN
            && clean
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.~+/=".contains(&b));
        valid.then(|| Self(clean.to_string()))
    }

    /// The raw value, for the HTTP header, the bridge XML and the vault only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ServerSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServerSecret(<redacted>)")
    }
}

/// Outgoing Stage 2 contract & direct-offer action types (`write/contractAction_*.xml` -> `POST /api/v1/contracts/actions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContractSubAction {
    CreateContract,
    CancelContract,
    AcceptContract,
    DeliverContract,
    CreateOffer,
    BuyOffer,
    CancelOffer,
    AckSettlement,
    RequestSync,
}

impl ContractSubAction {
    pub fn from_xml_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "CREATE_CONTRACT" => Some(Self::CreateContract),
            "CANCEL_CONTRACT" => Some(Self::CancelContract),
            "ACCEPT_CONTRACT" => Some(Self::AcceptContract),
            "DELIVER_CONTRACT" => Some(Self::DeliverContract),
            "CREATE_OFFER" => Some(Self::CreateOffer),
            "BUY_OFFER" => Some(Self::BuyOffer),
            "CANCEL_OFFER" => Some(Self::CancelOffer),
            "ACK_SETTLEMENT" => Some(Self::AckSettlement),
            "REQUEST_SYNC" => Some(Self::RequestSync),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CreateContract => "CREATE_CONTRACT",
            Self::CancelContract => "CANCEL_CONTRACT",
            Self::AcceptContract => "ACCEPT_CONTRACT",
            Self::DeliverContract => "DELIVER_CONTRACT",
            Self::CreateOffer => "CREATE_OFFER",
            Self::BuyOffer => "BUY_OFFER",
            Self::CancelOffer => "CANCEL_OFFER",
            Self::AckSettlement => "ACK_SETTLEMENT",
            Self::RequestSync => "REQUEST_SYNC",
        }
    }
}

/// Outgoing Stage 2 action parsed from `write/contractAction_<date>_<seq>.xml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractActionRequest {
    pub sub_action: ContractSubAction,
    pub action_id: String,
    pub server_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liters: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_trend_factor: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bonus_pct: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_delta_pct: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_partial: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_hours: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was_bale: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was_pallet: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_ids: Option<String>,
    /// Secret the mod sent along (`request#serverSecret`), `None` if it knows none yet.
    #[serde(skip)]
    pub server_secret: Option<ServerSecret>,
}

/// Authoritative online Supply Contract entry (`<contracts><contract .../></contracts>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineContractDto {
    pub remote_id: String,
    pub origin: String,
    pub is_npc: bool,
    pub issuer_server_id: String,
    pub issuer_farm_id: i32,
    pub issuer_name: String,
    pub contractor_server_id: String,
    pub contractor_farm_id: i32,
    pub fill_type: String,
    pub total_liters: i64,
    pub delivered_liters: i64,
    pub base_trend_factor: f32,
    pub bonus_pct: i32,
    pub remaining_seconds: i64,
    pub status: String,
}

/// Authoritative online +/-10% Direct Offer entry (`<directOffers><offer .../></directOffers>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineDirectOfferDto {
    pub remote_id: String,
    pub origin: String,
    pub is_npc: bool,
    pub seller_server_id: String,
    pub seller_farm_id: i32,
    pub seller_name: String,
    pub fill_type: String,
    pub total_liters: i64,
    pub remaining_liters: i64,
    pub price_delta_pct: i32,
    pub allow_partial: bool,
    pub remaining_seconds: i64,
    pub was_bale: bool,
    pub was_pallet: bool,
    pub status: String,
}

/// Wait-free `Pending -> Ack` result entry (`<actionResults><result .../></actionResults>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractActionResultDto {
    pub action_id: String,
    pub sub_action: String,
    pub status: String,
    pub reason: String,
    pub remote_id: String,
    pub granted_liters: i64,
    pub base_trend_factor: f32,
    pub bonus_pct: i32,
    pub remaining_seconds: i64,
}

/// Authoritative cross-server delivery, payout, refund, or breach-penalty settlement (`<settlements><settlement .../></settlements>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractSettlementDto {
    pub settlement_id: String,
    #[serde(rename = "type")]
    pub settlement_type: String,
    pub target_server_id: String,
    pub target_farm_id: i32,
    pub remote_id: String,
    pub fill_type: String,
    pub liters: i64,
    pub missing_liters: i64,
    pub base_trend_factor: f32,
    pub price_delta_pct: i32,
    pub source_label: String,
}

/// Complete Stage 2 contract & direct-offer state snapshot (`GET /api/v1/contracts/state` & `POST /api/v1/contracts/actions`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractStateResponse {
    pub updated_at: i64,
    pub contracts: Vec<OnlineContractDto>,
    pub direct_offers: Vec<OnlineDirectOfferDto>,
    pub action_results: Vec<ContractActionResultDto>,
    pub settlements: Vec<ContractSettlementDto>,
    /// Secret the backend issued in this response (once, on registration).
    #[serde(skip)]
    pub server_secret: Option<ServerSecret>,
}

/// Maximum number of `<config>` entries per vehicle (same limit as the backend, Spec Section 3.6).
pub const MAX_VEHICLE_CONFIGS: usize = 64;
/// Maximum size of one vehicle snapshot as JSON (same limit as the backend, Spec Section 3.6).
pub const MAX_VEHICLE_JSON_BYTES: usize = 16 * 1024;

/// Outgoing Stage 3 vehicle market action types (`write/vehicleAction_*.xml` -> `POST /api/v1/vehicles/actions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VehicleSubAction {
    CreateListing,
    CancelListing,
    BuyListing,
    AckSettlement,
    RequestSync,
}

impl VehicleSubAction {
    pub fn from_xml_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "CREATE_LISTING" => Some(Self::CreateListing),
            "CANCEL_LISTING" => Some(Self::CancelListing),
            "BUY_LISTING" => Some(Self::BuyListing),
            "ACK_SETTLEMENT" => Some(Self::AckSettlement),
            "REQUEST_SYNC" => Some(Self::RequestSync),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CreateListing => "CREATE_LISTING",
            Self::CancelListing => "CANCEL_LISTING",
            Self::BuyListing => "BUY_LISTING",
            Self::AckSettlement => "ACK_SETTLEMENT",
            Self::RequestSync => "REQUEST_SYNC",
        }
    }

    /// `REQUEST_SYNC` and `ACK_SETTLEMENT` run in the background and are not shown to the player.
    pub fn is_silent(self) -> bool {
        matches!(self, Self::RequestSync | Self::AckSettlement)
    }
}

/// One `<config>` of a vehicle snapshot (`vehicle.configurations[]`). The same `name` may appear
/// several times (one active entry, others only bought).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleConfigDto {
    pub name: String,
    pub save_id: String,
    pub is_active: bool,
    pub is_bought: bool,
    /// Custom paint `[r, g, b]` (XML `color="r g b"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material_template: Option<String>,
}

/// Vehicle snapshot (`<vehicle>` / `vehicle`), passed through as sent: the mod filters by compatibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleDto {
    /// Network form: `data/...`, `$pdlcdir$...` or `$moddir$FS25_Name/...`.
    pub xml_filename: String,
    /// `None` for basegame and DLC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mod_name: Option<String>,
    /// `None` for basegame and DLC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mod_file_hash: Option<String>,
    pub group: String,
    pub age_months: i64,
    pub operating_hours: f64,
    pub damage: f64,
    pub wear: f64,
    pub configurations: Vec<VehicleConfigDto>,
}

/// Outgoing Stage 3 action parsed from `write/vehicleAction_<date>_<seq>.xml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleActionRequest {
    pub sub_action: VehicleSubAction,
    pub action_id: String,
    pub server_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    /// `CANCEL_LISTING` of a listing whose `CREATE_LISTING` result was lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_action_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sale_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_factor: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_hours: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_ids: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<VehicleDto>,
    /// The `<vehicle>` had more than [`MAX_VEHICLE_CONFIGS`] `<config>` entries (the rest was dropped).
    #[serde(skip)]
    pub configs_truncated: bool,
    /// Secret the mod sent along (`request#serverSecret`), `None` if it knows none yet.
    #[serde(skip)]
    pub server_secret: Option<ServerSecret>,
}

impl VehicleActionRequest {
    /// Checks the limits the backend enforces before anything is sent, so an oversized or broken
    /// snapshot gets a `REJECTED` result (`INVALID_REQUEST`) and the mod releases its escrow.
    pub fn local_rejection_reason(&self) -> Option<&'static str> {
        if self.sub_action != VehicleSubAction::CreateListing {
            return None;
        }
        let Some(vehicle) = self.vehicle.as_ref() else {
            return Some("INVALID_REQUEST");
        };
        let too_big = self.configs_truncated
            || vehicle.configurations.len() > MAX_VEHICLE_CONFIGS
            || serde_json::to_vec(vehicle).map_or(true, |b| b.len() > MAX_VEHICLE_JSON_BYTES);
        (too_big || vehicle.xml_filename.trim().is_empty()).then_some("INVALID_REQUEST")
    }
}

/// Open online vehicle listing (`<listings><listing>`). Other savegames' `seller_server_id` and
/// `create_action_id` are `""` (masked by the backend) and are passed through unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineVehicleListingDto {
    pub remote_id: String,
    pub create_action_id: String,
    pub sale_mode: String,
    pub seller_server_id: String,
    pub seller_farm_id: i32,
    pub seller_name: String,
    pub price_factor: f64,
    pub remaining_seconds: i64,
    pub status: String,
    pub vehicle: VehicleDto,
}

/// Result of one vehicle action (`<actionResults><result>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleActionResultDto {
    pub action_id: String,
    pub sub_action: String,
    pub status: String,
    pub reason: String,
    pub remote_id: String,
    /// `CREATE_LISTING`: its own `action_id`; `CANCEL_LISTING`: the listing's.
    pub create_action_id: String,
    pub remaining_seconds: i64,
    /// Accepted `BUY_LISTING` and `CANCEL_LISTING` (except `NOT_CREATED`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<VehicleDto>,
}

/// Vehicle market settlement (`<settlements><settlement>`): `LISTING_SOLD`, `LISTING_BOUGHT`, `LISTING_EXPIRED`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleSettlementDto {
    pub settlement_id: String,
    #[serde(rename = "type")]
    pub settlement_type: String,
    pub target_server_id: String,
    pub target_farm_id: i32,
    pub remote_id: String,
    /// `LISTING_SOLD`, `LISTING_EXPIRED`; else `""`.
    pub create_action_id: String,
    /// `LISTING_BOUGHT`; else `""`.
    pub buy_action_id: String,
    pub price_factor: f64,
    /// `LISTING_SOLD`; else `""`.
    pub buyer_name: String,
    /// `LISTING_BOUGHT`; else `""`.
    pub seller_name: String,
    /// `LISTING_BOUGHT`, `LISTING_EXPIRED`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<VehicleDto>,
}

/// Complete Stage 3 vehicle market state (`GET /api/v1/vehicles/state` & `POST /api/v1/vehicles/actions`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VehicleStateResponse {
    pub updated_at: i64,
    pub listings: Vec<OnlineVehicleListingDto>,
    pub action_results: Vec<VehicleActionResultDto>,
    pub settlements: Vec<VehicleSettlementDto>,
    /// Secret the backend issued in this response (once, on registration; shared with contracts).
    #[serde(skip)]
    pub server_secret: Option<ServerSecret>,
}
