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
}
