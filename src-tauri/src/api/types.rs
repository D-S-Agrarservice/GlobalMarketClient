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
}
