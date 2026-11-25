//! Local in-memory simulation backend (`LocalMockBackend`).
//!
//! Allows testing 100% of the FS25 Lua mod online features locally in-game
//! (including `APPROVED` / `PENDING` / `REJECTED` moderation, authoritative
//! `greatDemand` triggers, buy/sell stock deltas, and `minVersion` force-update warnings)
//! without requiring a live FastAPI server.

mod vehicles;

use crate::api::types::{
    ContractActionRequest, ContractActionResultDto, ContractSettlementDto, ContractStateResponse,
    ContractSubAction, FillTypeStatus, MarketItemDto, MarketStateResponse, OnlineContractDto,
    OnlineDirectOfferDto, ServerHandshake, ServerNotice, ServerSecret, TransactionRequest,
    TransactionType, VehicleActionRequest, VehicleStateResponse, WhitelistSyncRequest,
};
use crate::api::{ApiError, BackendService};
use crate::bridge::xml::sanitize_market_name;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_CAPACITY_LITERS: f32 = 2_000_000.0;
const DEFAULT_MOCK_PRIVATE_MARKET_NAME: &str = "DSA-Dev Test (Mock)";
const MOCK_FOREIGN_SERVER_ID: &str = "mock_eu_srv_0001";
const FALLBACK_MOCK_LOGO_PNG: &[u8] = crate::bridge::dds::DEFAULT_DSA_LOGO_PNG;

#[derive(Debug, Clone)]
struct MockContractRecord {
    dto: OnlineContractDto,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct MockOfferRecord {
    dto: OnlineDirectOfferDto,
    expires_at: i64,
}

#[derive(Debug, Clone)]
struct MockActionResultRecord {
    server_id: String,
    created_at: i64,
    dto: ContractActionResultDto,
}

#[derive(Debug, Clone)]
struct MockSettlementRecord {
    acked: bool,
    dto: ContractSettlementDto,
}

#[derive(Debug)]
struct MockServerIdentity {
    secret: ServerSecret,
    confirmed: bool,
}

#[derive(Debug)]
struct MockState {
    simulate_offline: bool,
    simulated_contracts_supported: bool,
    simulated_min_version: String,
    simulated_latest_version: Option<String>,
    /// Simulated maintenance notice (Spec Section 3.1.3), `None` = no notice.
    simulated_notice: Option<ServerNotice>,
    /// Answer the status check with `503 MAINTENANCE` while `true`.
    simulate_maintenance: bool,
    simulated_branding_tier: u8,
    simulated_private_market_name: String,
    simulated_logo_url: Option<String>,
    simulated_logo_bytes: Option<Vec<u8>>,
    items: BTreeMap<String, MarketItemDto>,
    processed_tx_ids: Vec<String>,
    next_contract_seq: u64,
    next_offer_seq: u64,
    next_settlement_seq: u64,
    contracts: Vec<MockContractRecord>,
    direct_offers: Vec<MockOfferRecord>,
    action_results: BTreeMap<String, MockActionResultRecord>,
    settlements: Vec<MockSettlementRecord>,
    server_identities: BTreeMap<String, MockServerIdentity>,
    next_secret_seq: u64,
    /// Stage 3 used vehicle market (Spec Section 3.6).
    vehicles: vehicles::MockVehicleMarket,
}

impl Default for MockState {
    fn default() -> Self {
        let mut items = BTreeMap::new();
        for (ft, level) in [
            ("WHEAT", 1_350_000_i64),
            ("BARLEY", 820_000_i64),
            ("CANOLA", 540_000_i64),
            ("OAT", 410_000_i64),
            ("CORN", 960_000_i64),
            ("SUNFLOWER", 390_000_i64),
            ("SOYBEAN", 310_000_i64),
            ("SILAGE", 1_600_000_i64),
        ] {
            let trend = calculate_trend(level, DEFAULT_CAPACITY_LITERS);
            items.insert(
                ft.to_string(),
                MarketItemDto {
                    fill_type: ft.to_string(),
                    // Leave `title` unset until FS25 sends a map whitelist so UI & OS notifications
                    // localize default crops dynamically across DE/EN/FR/ES/PL.
                    title: None,
                    fill_level: level,
                    trend,
                    great_demand: false,
                    great_demand_bonus: 0.0,
                    great_demand_remaining: 0,
                    status: FillTypeStatus::Approved,
                },
            );
        }

        let now = now_unix_secs();
        let default_contracts = vec![MockContractRecord {
            dto: OnlineContractDto {
                remote_id: "mock_c_1".to_string(),
                origin: "ONLINE".to_string(),
                is_npc: false,
                issuer_server_id: MOCK_FOREIGN_SERVER_ID.to_string(),
                issuer_farm_id: 1,
                issuer_name: "Nordhof [Mock EU]".to_string(),
                contractor_server_id: String::new(),
                contractor_farm_id: 0,
                fill_type: "WHEAT".to_string(),
                total_liters: 50_000,
                delivered_liters: 0,
                base_trend_factor: 1.0,
                bonus_pct: 12,
                remaining_seconds: 172_800,
                status: "OPEN".to_string(),
            },
            expires_at: now + 172_800,
        }];
        let default_offers = vec![MockOfferRecord {
            dto: OnlineDirectOfferDto {
                remote_id: "mock_o_1".to_string(),
                origin: "ONLINE".to_string(),
                is_npc: false,
                seller_server_id: MOCK_FOREIGN_SERVER_ID.to_string(),
                seller_farm_id: 2,
                seller_name: "Südhof [Mock EU]".to_string(),
                fill_type: "BARLEY".to_string(),
                total_liters: 25_000,
                remaining_liters: 25_000,
                price_delta_pct: -5,
                allow_partial: true,
                remaining_seconds: 86_400,
                was_bale: false,
                was_pallet: false,
                status: "OPEN".to_string(),
            },
            expires_at: now + 86_400,
        }];

        Self {
            simulate_offline: false,
            simulated_contracts_supported: true,
            simulated_min_version: "0.9.12".to_string(),
            simulated_latest_version: None,
            simulated_notice: None,
            simulate_maintenance: false,
            simulated_branding_tier: 0,
            simulated_private_market_name: DEFAULT_MOCK_PRIVATE_MARKET_NAME.to_string(),
            simulated_logo_url: None,
            simulated_logo_bytes: None,
            items,
            processed_tx_ids: Vec::new(),
            next_contract_seq: 2,
            next_offer_seq: 2,
            next_settlement_seq: 1,
            contracts: default_contracts,
            direct_offers: default_offers,
            action_results: BTreeMap::new(),
            settlements: Vec::new(),
            server_identities: BTreeMap::new(),
            next_secret_seq: 0,
            vehicles: vehicles::MockVehicleMarket::default(),
        }
    }
}

pub struct LocalMockBackend {
    state: Mutex<MockState>,
}

impl Default for LocalMockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalMockBackend {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(MockState::default()),
        }
    }

    /// Toggle simulated backend outage (tests red HUD icon & seamless offline fallback in FS25).
    pub fn set_simulated_offline(&self, offline: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.simulate_offline = offline;
        }
    }

    /// Override `minVersion` reported to Lua (e.g. `"9.9.9"` to test `ui_gm_warning_exe_outdated`).
    pub fn set_simulated_min_version(&self, min_version: String) {
        if let Ok(mut guard) = self.state.lock() {
            guard.simulated_min_version = min_version;
        }
    }

    /// Override optional `latestVersion` ("nice-to-have" update announcement without blocking trading).
    pub fn set_simulated_latest_version(&self, latest_version: Option<String>) {
        if let Ok(mut guard) = self.state.lock() {
            guard.simulated_latest_version = latest_version
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
        }
    }

    /// Steps the simulated maintenance (Spec Section 3.1.3): `0` = none, `1` = maintenance announced
    /// (starts in 15 min, lasts 60 min), `2` = maintenance running (`503 MAINTENANCE`).
    /// Returns the notice that is now active.
    pub fn set_simulated_notice_stage(&self, stage: u8) -> Option<ServerNotice> {
        let Ok(mut guard) = self.state.lock() else {
            return None;
        };
        let now = unix_now_secs();
        match stage {
            0 => {
                guard.simulated_notice = None;
                guard.simulate_maintenance = false;
            }
            1 => {
                let message = BTreeMap::from([
                    ("de".to_string(), "Simulierte Wartung (Mock)".to_string()),
                    ("en".to_string(), "Simulated maintenance (Mock)".to_string()),
                ]);
                guard.simulated_notice = Some(ServerNotice {
                    id: format!("mock-maintenance-{now}"),
                    notice_type: "maintenance".to_string(),
                    starts_at: Some(now + 900),
                    ends_at: Some(now + 900 + 3600),
                    message,
                });
                guard.simulate_maintenance = false;
            }
            _ => {
                let mut notice = guard.simulated_notice.take().unwrap_or_else(|| ServerNotice {
                    id: format!("mock-maintenance-{now}"),
                    notice_type: "maintenance".to_string(),
                    starts_at: None,
                    ends_at: None,
                    message: BTreeMap::new(),
                });
                notice.starts_at = Some(now);
                notice.ends_at = Some(now + 3600);
                guard.simulated_notice = Some(notice);
                guard.simulate_maintenance = true;
            }
        }
        guard.simulated_notice.clone()
    }

    /// Set or clear a simulated Community Logo (`data:image/...;base64,...` + raw image bytes) in Mock mode.
    pub fn set_simulated_logo(&self, logo_url: Option<String>, logo_bytes: Option<Vec<u8>>) {
        if let Ok(mut guard) = self.state.lock() {
            let has_logo = logo_url.is_some() && logo_bytes.is_some();
            guard.simulated_logo_url = logo_url;
            guard.simulated_logo_bytes = logo_bytes;
            if has_logo {
                guard.simulated_branding_tier = match guard.simulated_branding_tier {
                    2 | 3 => 3,
                    _ => 1,
                };
            } else {
                guard.simulated_branding_tier = match guard.simulated_branding_tier {
                    2 | 3 => 2,
                    _ => 0,
                };
            }
        }
    }

    /// Overrides the simulated 3-tier market branding (`0..=3`) and optional private market name in Mock mode.
    pub fn set_simulated_branding_tier(&self, tier: u8, custom_name: Option<String>) {
        if let Ok(mut guard) = self.state.lock() {
            guard.simulated_branding_tier = tier.min(3);
            if let Some(raw) = custom_name {
                if let Some(clean) = sanitize_market_name(&raw) {
                    guard.simulated_private_market_name = clean;
                }
            }
            if matches!(guard.simulated_branding_tier, 1 | 3) && guard.simulated_logo_bytes.is_none()
            {
                let data_url = format!(
                    "data:image/png;base64,{}",
                    crate::bridge::coordinator::encode_base64(FALLBACK_MOCK_LOGO_PNG)
                );
                guard.simulated_logo_url = Some(data_url);
                guard.simulated_logo_bytes = Some(FALLBACK_MOCK_LOGO_PNG.to_vec());
            }
        }
    }

    /// Returns the currently active simulated branding tier (`0..=3`).
    pub fn simulated_branding_tier(&self) -> u8 {
        self.state
            .lock()
            .map(|g| g.simulated_branding_tier)
            .unwrap_or(0)
    }

    /// Returns the currently active simulated Community Logo URL/data-URI, if active in tier `1` or `3`.
    pub fn simulated_logo(&self) -> Option<String> {
        self.state.lock().ok().and_then(|guard| {
            if matches!(guard.simulated_branding_tier, 1 | 3) {
                guard.simulated_logo_url.clone()
            } else {
                None
            }
        })
    }

    /// Approve all currently `PENDING` mod fillTypes (tests seamless live transition in FS25).
    pub fn approve_all_pending(&self) -> usize {
        let Ok(mut guard) = self.state.lock() else {
            return 0;
        };
        let mut count = 0;
        for item in guard.items.values_mut() {
            if item.status == FillTypeStatus::Pending {
                item.status = FillTypeStatus::Approved;
                if item.fill_level == 0 {
                    item.fill_level = 500_000;
                }
                item.trend = calculate_trend(item.fill_level, DEFAULT_CAPACITY_LITERS);
                count += 1;
            }
        }
        count
    }

    /// Trigger or clear a simulated authoritative Great Demand on a specific (or first approved) FillType.
    pub fn trigger_great_demand(&self, target_fill_type: Option<&str>) -> Option<String> {
        let Ok(mut guard) = self.state.lock() else {
            return None;
        };
        let key = if let Some(ft) = target_fill_type {
            let upper = ft.trim().to_ascii_uppercase();
            guard.items.contains_key(&upper).then_some(upper)
        } else {
            guard
                .items
                .values()
                .find(|i| i.status == FillTypeStatus::Approved && !i.great_demand)
                .map(|i| i.fill_type.clone())
        }?;

        if let Some(item) = guard.items.get_mut(&key) {
            item.great_demand = true;
            item.great_demand_bonus = 0.25;
            item.great_demand_remaining = 200_000;
        }
        Some(key)
    }

    fn snapshot_locked(guard: &MockState) -> MarketStateResponse {
        MarketStateResponse {
            updated_at: now_unix_secs(),
            items: guard.items.values().cloned().collect(),
        }
    }

    /// Enables or disables Stage 2 `contracts_supported` in Mock mode.
    pub fn set_simulated_contracts_supported(&self, supported: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.simulated_contracts_supported = supported;
        }
    }

    /// Forces all currently open/active contracts and offers in Mock mode to expire immediately
    /// (used in integration tests and QA to verify `CONTRACT_EXPIRED_OPEN`, `CONTRACT_PENALTY_DEBIT`,
    /// `CONTRACT_DEFAULT_REFUND`, and `OFFER_RETURNED` settlements).
    pub fn expire_all_contracts_and_offers(&self) {
        if let Ok(mut guard) = self.state.lock() {
            let past = now_unix_secs() - 1;
            for c in &mut guard.contracts {
                if c.dto.status == "OPEN" || c.dto.status == "ACTIVE" {
                    c.expires_at = past;
                }
            }
            for o in &mut guard.direct_offers {
                if o.dto.status == "OPEN" {
                    o.expires_at = past;
                }
            }
            Self::process_expirations_locked(&mut guard, past + 1);
        }
    }

    /// Simulates foreign counterparty activity in Developer Mock mode:
    /// 1. Fulfils any player-published `OPEN` Supply Contracts (`CONTRACT_DELIVERY` + `CONTRACT_COMPLETED_ISSUER`).
    /// 2. Purchases any player-published `OPEN` Direct Offers (`OFFER_SOLD`).
    /// 3. Ensures at least one fresh foreign `OPEN` Contract and Direct Offer exist on the board.
    pub fn simulate_contract_activity(&self) -> String {
        let Ok(mut mutex_guard) = self.state.lock() else {
            return "mutex poisoned".to_string();
        };
        let guard = &mut *mutex_guard;
        let now = now_unix_secs();
        Self::process_expirations_locked(guard, now);

        let mut fulfilled_contracts = 0_usize;
        let mut bought_offers = 0_usize;
        let mut pending_settlements = Vec::new();

        for rec in &mut guard.contracts {
            if rec.dto.status == "OPEN" && rec.dto.issuer_server_id != MOCK_FOREIGN_SERVER_ID {
                let remaining = (rec.dto.total_liters - rec.dto.delivered_liters).max(0);
                if remaining > 0 {
                    rec.dto.contractor_server_id = MOCK_FOREIGN_SERVER_ID.to_string();
                    rec.dto.contractor_farm_id = 1;
                    rec.dto.delivered_liters = rec.dto.total_liters;
                    rec.dto.status = "COMPLETED".to_string();
                    fulfilled_contracts += 1;

                    let deliv_seq = guard.next_settlement_seq;
                    guard.next_settlement_seq += 1;
                    pending_settlements.push(MockSettlementRecord {
                        acked: false,
                        dto: ContractSettlementDto {
                            settlement_id: format!("mock_set_{deliv_seq}"),
                            settlement_type: "CONTRACT_DELIVERY".to_string(),
                            target_server_id: rec.dto.issuer_server_id.clone(),
                            target_farm_id: rec.dto.issuer_farm_id,
                            remote_id: rec.dto.remote_id.clone(),
                            fill_type: rec.dto.fill_type.clone(),
                            liters: remaining,
                            missing_liters: 0,
                            base_trend_factor: rec.dto.base_trend_factor,
                            price_delta_pct: 0,
                            source_label: format!("#{} (Hof Nordwind [Mock])", rec.dto.remote_id),
                        },
                    });

                    let comp_seq = guard.next_settlement_seq;
                    guard.next_settlement_seq += 1;
                    pending_settlements.push(MockSettlementRecord {
                        acked: false,
                        dto: ContractSettlementDto {
                            settlement_id: format!("mock_set_{comp_seq}"),
                            settlement_type: "CONTRACT_COMPLETED_ISSUER".to_string(),
                            target_server_id: rec.dto.issuer_server_id.clone(),
                            target_farm_id: rec.dto.issuer_farm_id,
                            remote_id: rec.dto.remote_id.clone(),
                            fill_type: rec.dto.fill_type.clone(),
                            liters: rec.dto.total_liters,
                            missing_liters: 0,
                            base_trend_factor: rec.dto.base_trend_factor,
                            price_delta_pct: 0,
                            source_label: format!("#{} (Hof Nordwind [Mock])", rec.dto.remote_id),
                        },
                    });
                }
            }
        }

        for rec in &mut guard.direct_offers {
            if rec.dto.status == "OPEN"
                && rec.dto.remaining_liters > 0
                && rec.dto.seller_server_id != MOCK_FOREIGN_SERVER_ID
            {
                let sold = rec.dto.remaining_liters;
                rec.dto.remaining_liters = 0;
                rec.dto.status = "SOLD_OUT".to_string();
                bought_offers += 1;

                let seq = guard.next_settlement_seq;
                guard.next_settlement_seq += 1;
                pending_settlements.push(MockSettlementRecord {
                    acked: false,
                    dto: ContractSettlementDto {
                        settlement_id: format!("mock_set_{seq}"),
                        settlement_type: "OFFER_SOLD".to_string(),
                        target_server_id: rec.dto.seller_server_id.clone(),
                        target_farm_id: rec.dto.seller_farm_id,
                        remote_id: rec.dto.remote_id.clone(),
                        fill_type: rec.dto.fill_type.clone(),
                        liters: sold,
                        missing_liters: 0,
                        base_trend_factor: 1.0,
                        price_delta_pct: rec.dto.price_delta_pct,
                        source_label: format!("#{} (Agrar Süd [Mock])", rec.dto.remote_id),
                    },
                });
            }
        }

        guard.settlements.extend(pending_settlements);

        let mut spawned_listings = 0_usize;
        let has_foreign_open_c = guard
            .contracts
            .iter()
            .any(|c| c.dto.status == "OPEN" && c.dto.issuer_server_id == MOCK_FOREIGN_SERVER_ID);
        if !has_foreign_open_c || (fulfilled_contracts == 0 && bought_offers == 0) {
            let c_seq = guard.next_contract_seq;
            guard.next_contract_seq += 1;
            let crops = ["WHEAT", "CANOLA", "CORN", "SOYBEAN"];
            let crop = crops[(c_seq as usize) % crops.len()];
            guard.contracts.push(MockContractRecord {
                dto: OnlineContractDto {
                    remote_id: format!("mock_c_{c_seq}"),
                    origin: "ONLINE".to_string(),
                    is_npc: false,
                    issuer_server_id: MOCK_FOREIGN_SERVER_ID.to_string(),
                    issuer_farm_id: 1,
                    issuer_name: "Nordhof [Mock EU]".to_string(),
                    contractor_server_id: String::new(),
                    contractor_farm_id: 0,
                    fill_type: crop.to_string(),
                    total_liters: 40_000,
                    delivered_liters: 0,
                    base_trend_factor: 1.0,
                    bonus_pct: 14,
                    remaining_seconds: 172_800,
                    status: "OPEN".to_string(),
                },
                expires_at: now + 172_800,
            });
            spawned_listings += 1;
        }

        let has_foreign_open_o = guard.direct_offers.iter().any(|o| {
            o.dto.status == "OPEN"
                && o.dto.remaining_liters > 0
                && o.dto.seller_server_id == MOCK_FOREIGN_SERVER_ID
        });
        if !has_foreign_open_o || (fulfilled_contracts == 0 && bought_offers == 0) {
            let o_seq = guard.next_offer_seq;
            guard.next_offer_seq += 1;
            let crops = ["BARLEY", "OAT", "SUNFLOWER", "SILAGE"];
            let crop = crops[(o_seq as usize) % crops.len()];
            guard.direct_offers.push(MockOfferRecord {
                dto: OnlineDirectOfferDto {
                    remote_id: format!("mock_o_{o_seq}"),
                    origin: "ONLINE".to_string(),
                    is_npc: false,
                    seller_server_id: MOCK_FOREIGN_SERVER_ID.to_string(),
                    seller_farm_id: 2,
                    seller_name: "Südhof [Mock EU]".to_string(),
                    fill_type: crop.to_string(),
                    total_liters: 30_000,
                    remaining_liters: 30_000,
                    price_delta_pct: -6,
                    allow_partial: true,
                    remaining_seconds: 86_400,
                    was_bale: false,
                    was_pallet: false,
                    status: "OPEN".to_string(),
                },
                expires_at: now + 86_400,
            });
            spawned_listings += 1;
        }

        format!(
            "fulfilled={fulfilled_contracts}, sold={bought_offers}, new_listings={spawned_listings}"
        )
    }

    fn process_expirations_locked(guard: &mut MockState, now: i64) {
        let mut new_settlements = Vec::new();

        for rec in &mut guard.contracts {
            if rec.expires_at <= now {
                if rec.dto.status == "OPEN" {
                    rec.dto.status = "EXPIRED".to_string();
                    rec.dto.remaining_seconds = 0;
                    if rec.dto.issuer_server_id != MOCK_FOREIGN_SERVER_ID {
                        let seq = guard.next_settlement_seq;
                        guard.next_settlement_seq += 1;
                        new_settlements.push(MockSettlementRecord {
                            acked: false,
                            dto: ContractSettlementDto {
                                settlement_id: format!("mock_set_{seq}"),
                                settlement_type: "CONTRACT_EXPIRED_OPEN".to_string(),
                                target_server_id: rec.dto.issuer_server_id.clone(),
                                target_farm_id: rec.dto.issuer_farm_id,
                                remote_id: rec.dto.remote_id.clone(),
                                fill_type: rec.dto.fill_type.clone(),
                                liters: rec.dto.total_liters,
                                missing_liters: 0,
                                base_trend_factor: rec.dto.base_trend_factor,
                                price_delta_pct: 0,
                                source_label: format!("#{}", rec.dto.remote_id),
                            },
                        });
                    }
                } else if rec.dto.status == "ACTIVE"
                    && rec.dto.delivered_liters < rec.dto.total_liters
                {
                    let missing = (rec.dto.total_liters - rec.dto.delivered_liters).max(0);
                    rec.dto.status = "DEFAULTED".to_string();
                    rec.dto.remaining_seconds = 0;

                    if !rec.dto.contractor_server_id.is_empty()
                        && rec.dto.contractor_server_id != MOCK_FOREIGN_SERVER_ID
                    {
                        let seq = guard.next_settlement_seq;
                        guard.next_settlement_seq += 1;
                        new_settlements.push(MockSettlementRecord {
                            acked: false,
                            dto: ContractSettlementDto {
                                settlement_id: format!("mock_set_{seq}"),
                                settlement_type: "CONTRACT_PENALTY_DEBIT".to_string(),
                                target_server_id: rec.dto.contractor_server_id.clone(),
                                target_farm_id: rec.dto.contractor_farm_id,
                                remote_id: rec.dto.remote_id.clone(),
                                fill_type: rec.dto.fill_type.clone(),
                                liters: 0,
                                missing_liters: missing,
                                base_trend_factor: rec.dto.base_trend_factor,
                                price_delta_pct: 0,
                                source_label: format!("#{}", rec.dto.remote_id),
                            },
                        });
                    }
                    if rec.dto.issuer_server_id != MOCK_FOREIGN_SERVER_ID {
                        let seq = guard.next_settlement_seq;
                        guard.next_settlement_seq += 1;
                        new_settlements.push(MockSettlementRecord {
                            acked: false,
                            dto: ContractSettlementDto {
                                settlement_id: format!("mock_set_{seq}"),
                                settlement_type: "CONTRACT_DEFAULT_REFUND".to_string(),
                                target_server_id: rec.dto.issuer_server_id.clone(),
                                target_farm_id: rec.dto.issuer_farm_id,
                                remote_id: rec.dto.remote_id.clone(),
                                fill_type: rec.dto.fill_type.clone(),
                                liters: 0,
                                missing_liters: missing,
                                base_trend_factor: rec.dto.base_trend_factor,
                                price_delta_pct: 0,
                                source_label: format!("#{}", rec.dto.remote_id),
                            },
                        });
                    }
                }
            }
        }

        for rec in &mut guard.direct_offers {
            if rec.expires_at <= now && rec.dto.status == "OPEN" && rec.dto.remaining_liters > 0 {
                let rem = rec.dto.remaining_liters;
                rec.dto.status = "EXPIRED".to_string();
                rec.dto.remaining_seconds = 0;
                if rec.dto.seller_server_id != MOCK_FOREIGN_SERVER_ID {
                    let seq = guard.next_settlement_seq;
                    guard.next_settlement_seq += 1;
                    new_settlements.push(MockSettlementRecord {
                        acked: false,
                        dto: ContractSettlementDto {
                            settlement_id: format!("mock_set_{seq}"),
                            settlement_type: "OFFER_RETURNED".to_string(),
                            target_server_id: rec.dto.seller_server_id.clone(),
                            target_farm_id: rec.dto.seller_farm_id,
                            remote_id: rec.dto.remote_id.clone(),
                            fill_type: rec.dto.fill_type.clone(),
                            liters: rem,
                            missing_liters: 0,
                            base_trend_factor: 1.0,
                            price_delta_pct: rec.dto.price_delta_pct,
                            source_label: format!("#{}", rec.dto.remote_id),
                        },
                    });
                }
            }
        }

        guard.settlements.extend(new_settlements);
    }

    fn contract_snapshot_locked(
        guard: &mut MockState,
        server_id: Option<&str>,
    ) -> ContractStateResponse {
        let now = now_unix_secs();
        Self::process_expirations_locked(guard, now);
        let sid = server_id.map(str::trim).filter(|s| !s.is_empty());
        // Like the backend: other savegames' server ids are never sent.
        let mask = |id: &str| -> String {
            if sid.is_some_and(|s| s == id) {
                id.to_string()
            } else {
                String::new()
            }
        };

        let contracts = guard
            .contracts
            .iter()
            .filter(|rec| {
                rec.dto.status == "OPEN"
                    || (rec.dto.status == "ACTIVE"
                        && sid.is_some_and(|s| {
                            rec.dto.issuer_server_id == s || rec.dto.contractor_server_id == s
                        }))
            })
            .map(|rec| {
                let mut dto = rec.dto.clone();
                dto.remaining_seconds = (rec.expires_at - now).max(0);
                dto.issuer_server_id = mask(&dto.issuer_server_id);
                dto.contractor_server_id = mask(&dto.contractor_server_id);
                dto
            })
            .collect();

        let direct_offers = guard
            .direct_offers
            .iter()
            .filter(|rec| rec.dto.status == "OPEN" && rec.dto.remaining_liters > 0)
            .map(|rec| {
                let mut dto = rec.dto.clone();
                dto.remaining_seconds = (rec.expires_at - now).max(0);
                dto.seller_server_id = mask(&dto.seller_server_id);
                dto
            })
            .collect();

        let action_results = match sid {
            Some(s) => guard
                .action_results
                .values()
                .filter(|r| r.server_id == s && (now - r.created_at) <= 600)
                .map(|r| r.dto.clone())
                .collect(),
            None => Vec::new(),
        };

        let settlements = match sid {
            Some(s) => guard
                .settlements
                .iter()
                .filter(|rec| !rec.acked && rec.dto.target_server_id == s)
                .map(|rec| rec.dto.clone())
                .collect(),
            None => Vec::new(),
        };

        ContractStateResponse {
            updated_at: now,
            contracts,
            direct_offers,
            action_results,
            settlements,
            server_secret: None,
        }
    }
}

impl BackendService for LocalMockBackend {
    fn check_status(&self) -> Result<ServerHandshake, ApiError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;

        if guard.simulate_offline {
            return Err(ApiError::Network(
                "Simulated connection outage (Mock mode)".to_string(),
            ));
        }
        if guard.simulate_maintenance {
            let notice = guard.simulated_notice.as_ref();
            return Err(ApiError::Maintenance {
                notice_id: notice.map(|n| n.id.clone()),
                until: notice.and_then(|n| n.ends_at),
            });
        }

        let tier = guard.simulated_branding_tier.min(3);
        let is_private_market = matches!(tier, 2 | 3);
        let market_name = if is_private_market {
            guard.simulated_private_market_name.clone()
        } else {
            "Local Simulation (Mock)".to_string()
        };
        let market_logo_url = if matches!(tier, 1 | 3) {
            guard.simulated_logo_url.clone()
        } else {
            None
        };

        Ok(ServerHandshake {
            online: true,
            min_exe_version: guard.simulated_min_version.clone(),
            latest_exe_version: guard.simulated_latest_version.clone(),
            poll_interval_sec: 5,
            market_name,
            api_version: "2.0.0-mock".to_string(),
            branding_tier: tier,
            is_private_market,
            market_logo_url,
            contracts_supported: guard.simulated_contracts_supported,
            vehicles_supported: guard.vehicles.supported,
            notice: guard.simulated_notice.clone(),
        })
    }

    fn sync_whitelist(&self, req: WhitelistSyncRequest) -> Result<MarketStateResponse, ApiError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;

        if guard.simulate_offline {
            return Err(ApiError::Network(
                "Simulated connection outage (Mock mode)".to_string(),
            ));
        }

        for wl in req.items {
            let key = wl.name.trim().to_ascii_uppercase();
            if key.is_empty() {
                continue;
            }
            guard
                .items
                .entry(key.clone())
                .and_modify(|existing| {
                    if !wl.title.trim().is_empty() {
                        existing.title = Some(wl.title.clone());
                    }
                })
                .or_insert_with(|| {
                    let is_basegame = wl.source.eq_ignore_ascii_case("BASEGAME");
                    let initial_level = if is_basegame { 750_000_i64 } else { 0_i64 };
                    let status = if is_basegame {
                        FillTypeStatus::Approved
                    } else {
                        FillTypeStatus::Pending
                    };
                    MarketItemDto {
                        fill_type: key,
                        title: Some(wl.title),
                        fill_level: initial_level,
                        trend: if is_basegame {
                            calculate_trend(initial_level, DEFAULT_CAPACITY_LITERS)
                        } else {
                            1.0
                        },
                        great_demand: false,
                        great_demand_bonus: 0.0,
                        great_demand_remaining: 0,
                        status,
                    }
                });
        }

        Ok(Self::snapshot_locked(&guard))
    }

    fn fetch_market_state(&self) -> Result<MarketStateResponse, ApiError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;

        if guard.simulate_offline {
            return Err(ApiError::Network(
                "Simulated connection outage (Mock mode)".to_string(),
            ));
        }

        Ok(Self::snapshot_locked(&guard))
    }

    fn submit_transaction(&self, tx: TransactionRequest) -> Result<MarketStateResponse, ApiError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;

        if guard.simulate_offline {
            return Err(ApiError::Network(
                "Simulated connection outage (Mock mode)".to_string(),
            ));
        }

        // Idempotency guard against duplicate submission
        if !tx.tx_id.is_empty() && guard.processed_tx_ids.contains(&tx.tx_id) {
            return Ok(Self::snapshot_locked(&guard));
        }

        let key = tx.fill_type.trim().to_ascii_uppercase();
        let amount = tx.amount.max(0);

        if let Some(item) = guard.items.get_mut(&key) {
            if item.status == FillTypeStatus::Approved && amount > 0 {
                match tx.tx_type {
                    TransactionType::Buy => {
                        item.fill_level = (item.fill_level - amount).max(0);
                    }
                    TransactionType::Sell => {
                        item.fill_level = item.fill_level.saturating_add(amount);
                        if item.great_demand {
                            item.great_demand_remaining =
                                (item.great_demand_remaining - amount).max(0);
                            if item.great_demand_remaining == 0 {
                                item.great_demand = false;
                                item.great_demand_bonus = 0.0;
                            }
                        }
                    }
                }
                item.trend = calculate_trend(item.fill_level, DEFAULT_CAPACITY_LITERS);
            }
        }

        if !tx.tx_id.is_empty() {
            guard.processed_tx_ids.push(tx.tx_id);
            if guard.processed_tx_ids.len() > 256 {
                guard.processed_tx_ids.remove(0);
            }
        }

        Ok(Self::snapshot_locked(&guard))
    }

    fn fetch_contracts_state(
        &self,
        server_id: Option<&str>,
        server_secret: Option<&ServerSecret>,
    ) -> Result<ContractStateResponse, ApiError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;

        if guard.simulate_offline {
            return Err(ApiError::Network(
                "Simulated connection outage (Mock mode)".to_string(),
            ));
        }
        if !guard.simulated_contracts_supported {
            return Err(ApiError::NotSupported(
                "Contracts disabled in Mock mode".to_string(),
            ));
        }

        // Like the backend for reads: unknown savegames get the board only, known ones need the secret.
        let mut own = None;
        if let Some(sid) = server_id.map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(identity) = guard.server_identities.get_mut(&sid.to_ascii_lowercase()) {
                if server_secret != Some(&identity.secret) {
                    return Err(ApiError::ServerSecretInvalid);
                }
                identity.confirmed = true;
                own = Some(sid);
            }
        }
        Ok(Self::contract_snapshot_locked(&mut guard, own))
    }

    fn submit_contract_action(
        &self,
        action: ContractActionRequest,
        server_secret: Option<&ServerSecret>,
    ) -> Result<ContractStateResponse, ApiError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;

        if guard.simulate_offline {
            return Err(ApiError::Network(
                "Simulated connection outage (Mock mode)".to_string(),
            ));
        }
        if !guard.simulated_contracts_supported {
            return Err(ApiError::NotSupported(
                "Contracts disabled in Mock mode".to_string(),
            ));
        }

        let now = now_unix_secs();
        Self::process_expirations_locked(&mut guard, now);

        let server_id = action.server_id.trim().to_string();
        if action.action_id.trim().is_empty() || server_id.is_empty() {
            return Err(ApiError::Validation(
                "action_id and server_id are required".to_string(),
            ));
        }
        let issued = Self::authenticate_or_register_locked(&mut guard, &server_id, server_secret)?;
        let mut state = Self::apply_contract_action_locked(&mut guard, now, action);
        state.server_secret = issued;
        Ok(state)
    }

    fn fetch_vehicles_state(
        &self,
        server_id: Option<&str>,
        server_secret: Option<&ServerSecret>,
    ) -> Result<VehicleStateResponse, ApiError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;
        if guard.simulate_offline {
            return Err(ApiError::Network(
                "Simulated connection outage (Mock mode)".to_string(),
            ));
        }
        if !guard.vehicles.supported {
            return Err(ApiError::NotSupported(
                "Vehicle market disabled in Mock mode".to_string(),
            ));
        }
        // Same savegame registry as contracts: unknown savegames get the board only.
        let mut own = None;
        if let Some(sid) = server_id.map(str::trim).filter(|s| !s.is_empty()) {
            let key = sid.to_ascii_lowercase();
            if let Some(identity) = guard.server_identities.get_mut(&key) {
                if server_secret != Some(&identity.secret) {
                    return Err(ApiError::ServerSecretInvalid);
                }
                identity.confirmed = true;
                own = Some(key);
            }
        }
        Ok(Self::vehicles_snapshot_locked(&mut guard, own.as_deref()))
    }

    fn submit_vehicle_action(
        &self,
        action: VehicleActionRequest,
        server_secret: Option<&ServerSecret>,
    ) -> Result<VehicleStateResponse, ApiError> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;
        if guard.simulate_offline {
            return Err(ApiError::Network(
                "Simulated connection outage (Mock mode)".to_string(),
            ));
        }
        if !guard.vehicles.supported {
            return Err(ApiError::NotSupported(
                "Vehicle market disabled in Mock mode".to_string(),
            ));
        }
        let server_id = action.server_id.trim().to_string();
        if action.action_id.trim().is_empty() || server_id.is_empty() {
            return Err(ApiError::Validation(
                "action_id and server_id are required".to_string(),
            ));
        }
        let issued = Self::authenticate_or_register_locked(&mut guard, &server_id, server_secret)?;
        let mut state = guard.vehicles.apply_action(now_unix_secs(), action);
        state.server_secret = issued;
        Ok(state)
    }

    fn fetch_logo_bytes(&self, _logo_url: &str) -> Result<Vec<u8>, ApiError> {
        let guard = self
            .state
            .lock()
            .map_err(|_| ApiError::Internal("Mock mutex poisoned".to_string()))?;
        guard
            .simulated_logo_bytes
            .clone()
            .ok_or_else(|| ApiError::Validation("No simulated logo configured in Mock mode".to_string()))
    }
}

impl LocalMockBackend {
    /// Same rules as the backend for actions: registers an unknown savegame, issues a new secret until the
    /// first one was used, otherwise requires the secret. A secret the savegame already has is adopted on
    /// first contact, so testing in Mock mode never replaces the real one in the savegame.
    fn authenticate_or_register_locked(
        guard: &mut MockState,
        server_id: &str,
        secret: Option<&ServerSecret>,
    ) -> Result<Option<ServerSecret>, ApiError> {
        let key = server_id.to_ascii_lowercase();
        if let Some(identity) = guard.server_identities.get_mut(&key) {
            if secret == Some(&identity.secret) {
                identity.confirmed = true;
                return Ok(None);
            }
            if identity.confirmed {
                return Err(ApiError::ServerSecretInvalid);
            }
        } else if let Some(existing) = secret {
            guard.server_identities.insert(
                key,
                MockServerIdentity {
                    secret: existing.clone(),
                    confirmed: true,
                },
            );
            return Ok(None);
        }
        guard.next_secret_seq += 1;
        let issued = ServerSecret::parse(&format!(
            "mock-{:x}-{}",
            now_unix_secs(),
            guard.next_secret_seq
        ));
        if let Some(ref secret) = issued {
            guard.server_identities.insert(
                key,
                MockServerIdentity {
                    secret: secret.clone(),
                    confirmed: false,
                },
            );
        }
        Ok(issued)
    }

    /// Forgets all registered savegames, like a secret reset by support.
    pub fn reset_server_identities(&self) {
        if let Ok(mut guard) = self.state.lock() {
            guard.server_identities.clear();
        }
    }

    fn apply_contract_action_locked(
        guard: &mut MockState,
        now: i64,
        action: ContractActionRequest,
    ) -> ContractStateResponse {
        let action_id = action.action_id.trim().to_string();
        let server_id = action.server_id.trim().to_string();

        // Idempotent replay: if `action_id` was already processed, return current snapshot (including stored result) without mutating twice.
        if guard.action_results.contains_key(&action_id) {
            return Self::contract_snapshot_locked(guard, Some(&server_id));
        }

        match action.sub_action {
            ContractSubAction::RequestSync => {
                return Self::contract_snapshot_locked(guard, Some(&server_id));
            }
            ContractSubAction::AckSettlement => {
                if let Some(ref raw_ids) = action.settlement_ids {
                    for sid in raw_ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        for rec in &mut guard.settlements {
                            if rec.dto.settlement_id == sid && rec.dto.target_server_id == server_id
                            {
                                rec.acked = true;
                            }
                        }
                    }
                }
                return Self::contract_snapshot_locked(guard, Some(&server_id));
            }
            ContractSubAction::CreateContract => {
                let ft = action
                    .fill_type
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_uppercase();
                let liters = action.liters.unwrap_or(0);
                let bonus = action.bonus_pct.unwrap_or(0);
                let hours = action.duration_hours.unwrap_or(0);
                let farm_id = action.farm_id.unwrap_or(0);
                let factor = action
                    .base_trend_factor
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .unwrap_or(1.0);
                let approved = guard
                    .items
                    .get(&ft)
                    .is_some_and(|i| i.status == FillTypeStatus::Approved);

                let result = if !approved {
                    ContractActionResultDto {
                        action_id: action_id.clone(),
                        sub_action: "CREATE_CONTRACT".to_string(),
                        status: "REJECTED".to_string(),
                        reason: "FILLTYPE_NOT_APPROVED".to_string(),
                        remote_id: String::new(),
                        granted_liters: 0,
                        base_trend_factor: factor,
                        bonus_pct: bonus,
                        remaining_seconds: 0,
                    }
                } else if liters < 1_000
                    || !(5..=20).contains(&bonus)
                    || !(1..=168).contains(&hours)
                    || farm_id <= 0
                {
                    ContractActionResultDto {
                        action_id: action_id.clone(),
                        sub_action: "CREATE_CONTRACT".to_string(),
                        status: "REJECTED".to_string(),
                        reason: "INVALID_CONTRACT_PARAMS".to_string(),
                        remote_id: String::new(),
                        granted_liters: 0,
                        base_trend_factor: factor,
                        bonus_pct: bonus,
                        remaining_seconds: 0,
                    }
                } else {
                    let seq = guard.next_contract_seq;
                    guard.next_contract_seq += 1;
                    let remote_id = format!("mock_c_{seq}");
                    let rem_secs = (hours as i64) * 3_600;
                    let issuer_name = action
                        .farm_name
                        .as_deref()
                        .and_then(sanitize_market_name)
                        .unwrap_or_else(|| format!("Farm #{farm_id}"));
                    guard.contracts.push(MockContractRecord {
                        dto: OnlineContractDto {
                            remote_id: remote_id.clone(),
                            origin: "ONLINE".to_string(),
                            is_npc: false,
                            issuer_server_id: server_id.clone(),
                            issuer_farm_id: farm_id,
                            issuer_name,
                            contractor_server_id: String::new(),
                            contractor_farm_id: 0,
                            fill_type: ft,
                            total_liters: liters,
                            delivered_liters: 0,
                            base_trend_factor: factor,
                            bonus_pct: bonus,
                            remaining_seconds: rem_secs,
                            status: "OPEN".to_string(),
                        },
                        expires_at: now + rem_secs,
                    });
                    ContractActionResultDto {
                        action_id: action_id.clone(),
                        sub_action: "CREATE_CONTRACT".to_string(),
                        status: "ACCEPTED".to_string(),
                        reason: String::new(),
                        remote_id,
                        granted_liters: 0,
                        base_trend_factor: factor,
                        bonus_pct: bonus,
                        remaining_seconds: rem_secs,
                    }
                };
                Self::store_action_result_locked(guard, &server_id, now, result);
            }
            ContractSubAction::CancelContract => {
                let rid = action.remote_id.as_deref().unwrap_or("").trim();
                let farm_id = action.farm_id.unwrap_or(0);
                let mut accepted = false;
                for rec in &mut guard.contracts {
                    if rec.dto.remote_id == rid
                        && rec.dto.issuer_server_id == server_id
                        && rec.dto.issuer_farm_id == farm_id
                        && rec.dto.status == "OPEN"
                    {
                        rec.dto.status = "CANCELLED".to_string();
                        accepted = true;
                        break;
                    }
                }
                Self::store_action_result_locked(
                    guard,
                    &server_id,
                    now,
                    ContractActionResultDto {
                        action_id,
                        sub_action: "CANCEL_CONTRACT".to_string(),
                        status: if accepted { "ACCEPTED" } else { "REJECTED" }.to_string(),
                        reason: if accepted {
                            String::new()
                        } else {
                            "NOT_OPEN_OR_NOT_OWNER".to_string()
                        },
                        remote_id: rid.to_string(),
                        granted_liters: 0,
                        base_trend_factor: 1.0,
                        bonus_pct: 0,
                        remaining_seconds: 0,
                    },
                );
            }
            ContractSubAction::AcceptContract => {
                let rid = action.remote_id.as_deref().unwrap_or("").trim();
                let farm_id = action.farm_id.unwrap_or(0);
                let mut accepted_factor: Option<(f32, i32, i64)> = None;
                if farm_id > 0 {
                    for rec in &mut guard.contracts {
                        let is_self = rec.dto.issuer_server_id == server_id
                            && rec.dto.issuer_farm_id == farm_id;
                        if rec.dto.remote_id == rid && rec.dto.status == "OPEN" && !is_self {
                            rec.dto.status = "ACTIVE".to_string();
                            rec.dto.contractor_server_id = server_id.clone();
                            rec.dto.contractor_farm_id = farm_id;
                            let rem_secs = (rec.expires_at - now).max(0);
                            accepted_factor =
                                Some((rec.dto.base_trend_factor, rec.dto.bonus_pct, rem_secs));
                            break;
                        }
                    }
                }
                let result = match accepted_factor {
                    Some((factor, bonus, rem_secs)) => ContractActionResultDto {
                        action_id,
                        sub_action: "ACCEPT_CONTRACT".to_string(),
                        status: "ACCEPTED".to_string(),
                        reason: String::new(),
                        remote_id: rid.to_string(),
                        granted_liters: 0,
                        base_trend_factor: factor,
                        bonus_pct: bonus,
                        remaining_seconds: rem_secs,
                    },
                    None => ContractActionResultDto {
                        action_id,
                        sub_action: "ACCEPT_CONTRACT".to_string(),
                        status: "REJECTED".to_string(),
                        reason: "CONTRACT_UNAVAILABLE".to_string(),
                        remote_id: rid.to_string(),
                        granted_liters: 0,
                        base_trend_factor: 1.0,
                        bonus_pct: 0,
                        remaining_seconds: 0,
                    },
                };
                Self::store_action_result_locked(guard, &server_id, now, result);
            }
            ContractSubAction::DeliverContract => {
                let rid = action.remote_id.as_deref().unwrap_or("").trim();
                let farm_id = action.farm_id.unwrap_or(0);
                let req_liters = action.liters.unwrap_or(0);
                let contractor_label = action
                    .farm_name
                    .as_deref()
                    .and_then(sanitize_market_name)
                    .unwrap_or_else(|| format!("Farm #{farm_id}"));

                let mut credited = 0_i64;
                let mut delivery_info: Option<(String, i32, String, f32, bool, i64)> = None;

                if req_liters > 0 {
                    for rec in &mut guard.contracts {
                        if rec.dto.remote_id == rid
                            && rec.dto.status == "ACTIVE"
                            && rec.dto.contractor_server_id == server_id
                            && rec.dto.contractor_farm_id == farm_id
                        {
                            let needed = (rec.dto.total_liters - rec.dto.delivered_liters).max(0);
                            credited = req_liters.min(needed);
                            rec.dto.delivered_liters += credited;
                            let completed = rec.dto.delivered_liters >= rec.dto.total_liters;
                            if completed {
                                rec.dto.status = "COMPLETED".to_string();
                            }
                            delivery_info = Some((
                                rec.dto.issuer_server_id.clone(),
                                rec.dto.issuer_farm_id,
                                rec.dto.fill_type.clone(),
                                rec.dto.base_trend_factor,
                                completed,
                                rec.dto.total_liters,
                            ));
                            break;
                        }
                    }
                }

                if let Some((issuer_sid, issuer_fid, ft, factor, completed, total_liters)) =
                    delivery_info
                {
                    if credited > 0 && issuer_sid != MOCK_FOREIGN_SERVER_ID {
                        let seq = guard.next_settlement_seq;
                        guard.next_settlement_seq += 1;
                        guard.settlements.push(MockSettlementRecord {
                            acked: false,
                            dto: ContractSettlementDto {
                                settlement_id: format!("mock_set_{seq}"),
                                settlement_type: "CONTRACT_DELIVERY".to_string(),
                                target_server_id: issuer_sid.clone(),
                                target_farm_id: issuer_fid,
                                remote_id: rid.to_string(),
                                fill_type: ft.clone(),
                                liters: credited,
                                missing_liters: 0,
                                base_trend_factor: factor,
                                price_delta_pct: 0,
                                source_label: format!("#{rid} ({contractor_label})"),
                            },
                        });
                        if completed {
                            let cseq = guard.next_settlement_seq;
                            guard.next_settlement_seq += 1;
                            guard.settlements.push(MockSettlementRecord {
                                acked: false,
                                dto: ContractSettlementDto {
                                    settlement_id: format!("mock_set_{cseq}"),
                                    settlement_type: "CONTRACT_COMPLETED_ISSUER".to_string(),
                                    target_server_id: issuer_sid,
                                    target_farm_id: issuer_fid,
                                    remote_id: rid.to_string(),
                                    fill_type: ft,
                                    liters: total_liters,
                                    missing_liters: 0,
                                    base_trend_factor: factor,
                                    price_delta_pct: 0,
                                    source_label: format!("#{rid} ({contractor_label})"),
                                },
                            });
                        }
                    }
                    Self::store_action_result_locked(
                        guard,
                        &server_id,
                        now,
                        ContractActionResultDto {
                            action_id,
                            sub_action: "DELIVER_CONTRACT".to_string(),
                            status: "ACCEPTED".to_string(),
                            reason: String::new(),
                            remote_id: rid.to_string(),
                            granted_liters: credited,
                            base_trend_factor: factor,
                            bonus_pct: 0,
                            remaining_seconds: 0,
                        },
                    );
                } else {
                    Self::store_action_result_locked(
                        guard,
                        &server_id,
                        now,
                        ContractActionResultDto {
                            action_id,
                            sub_action: "DELIVER_CONTRACT".to_string(),
                            status: "REJECTED".to_string(),
                            reason: "CONTRACT_NOT_ACTIVE".to_string(),
                            remote_id: rid.to_string(),
                            granted_liters: 0,
                            base_trend_factor: 1.0,
                            bonus_pct: 0,
                            remaining_seconds: 0,
                        },
                    );
                }
            }
            ContractSubAction::CreateOffer => {
                let ft = action
                    .fill_type
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_uppercase();
                let liters = action.liters.unwrap_or(0);
                let delta = action.price_delta_pct.unwrap_or(0);
                let hours = action.duration_hours.unwrap_or(0);
                let farm_id = action.farm_id.unwrap_or(0);
                let allow_partial = action.allow_partial.unwrap_or(true);
                let was_bale = action.was_bale.unwrap_or(false);
                let was_pallet = action.was_pallet.unwrap_or(false);
                let approved = guard
                    .items
                    .get(&ft)
                    .is_some_and(|i| i.status == FillTypeStatus::Approved);

                let result = if !approved {
                    ContractActionResultDto {
                        action_id,
                        sub_action: "CREATE_OFFER".to_string(),
                        status: "REJECTED".to_string(),
                        reason: "FILLTYPE_NOT_APPROVED".to_string(),
                        remote_id: String::new(),
                        granted_liters: 0,
                        base_trend_factor: 1.0,
                        bonus_pct: 0,
                        remaining_seconds: 0,
                    }
                } else if liters <= 0
                    || !(-10..=10).contains(&delta)
                    || !(1..=72).contains(&hours)
                    || farm_id <= 0
                {
                    ContractActionResultDto {
                        action_id,
                        sub_action: "CREATE_OFFER".to_string(),
                        status: "REJECTED".to_string(),
                        reason: "INVALID_OFFER_PARAMS".to_string(),
                        remote_id: String::new(),
                        granted_liters: 0,
                        base_trend_factor: 1.0,
                        bonus_pct: 0,
                        remaining_seconds: 0,
                    }
                } else {
                    let seq = guard.next_offer_seq;
                    guard.next_offer_seq += 1;
                    let remote_id = format!("mock_o_{seq}");
                    let rem_secs = (hours as i64) * 3_600;
                    let seller_name = action
                        .farm_name
                        .as_deref()
                        .and_then(sanitize_market_name)
                        .unwrap_or_else(|| format!("Farm #{farm_id}"));
                    guard.direct_offers.push(MockOfferRecord {
                        dto: OnlineDirectOfferDto {
                            remote_id: remote_id.clone(),
                            origin: "ONLINE".to_string(),
                            is_npc: false,
                            seller_server_id: server_id.clone(),
                            seller_farm_id: farm_id,
                            seller_name,
                            fill_type: ft,
                            total_liters: liters,
                            remaining_liters: liters,
                            price_delta_pct: delta,
                            allow_partial,
                            remaining_seconds: rem_secs,
                            was_bale,
                            was_pallet,
                            status: "OPEN".to_string(),
                        },
                        expires_at: now + rem_secs,
                    });
                    ContractActionResultDto {
                        action_id,
                        sub_action: "CREATE_OFFER".to_string(),
                        status: "ACCEPTED".to_string(),
                        reason: String::new(),
                        remote_id,
                        granted_liters: 0,
                        base_trend_factor: 1.0,
                        bonus_pct: 0,
                        remaining_seconds: rem_secs,
                    }
                };
                Self::store_action_result_locked(guard, &server_id, now, result);
            }
            ContractSubAction::BuyOffer => {
                let rid = action.remote_id.as_deref().unwrap_or("").trim();
                let buyer_farm_id = action.farm_id.unwrap_or(0);
                let req_liters = action.liters.unwrap_or(0);
                let mut granted = 0_i64;
                let mut sold_info: Option<(String, i32, String, i32)> = None;

                if buyer_farm_id > 0 && req_liters > 0 {
                    for rec in &mut guard.direct_offers {
                        let is_self = rec.dto.seller_server_id == server_id
                            && rec.dto.seller_farm_id == buyer_farm_id;
                        if rec.dto.remote_id == rid
                            && rec.dto.status == "OPEN"
                            && rec.dto.remaining_liters > 0
                            && !is_self
                        {
                            if !rec.dto.allow_partial && rec.dto.remaining_liters < req_liters {
                                break;
                            }
                            granted = if rec.dto.allow_partial {
                                req_liters.min(rec.dto.remaining_liters)
                            } else {
                                req_liters.min(rec.dto.remaining_liters)
                            };
                            rec.dto.remaining_liters -= granted;
                            if rec.dto.remaining_liters == 0 {
                                rec.dto.status = "SOLD_OUT".to_string();
                            }
                            sold_info = Some((
                                rec.dto.seller_server_id.clone(),
                                rec.dto.seller_farm_id,
                                rec.dto.fill_type.clone(),
                                rec.dto.price_delta_pct,
                            ));
                            break;
                        }
                    }
                }

                if let Some((seller_sid, seller_fid, ft, delta)) = sold_info {
                    if granted > 0 && seller_sid != MOCK_FOREIGN_SERVER_ID {
                        let seq = guard.next_settlement_seq;
                        guard.next_settlement_seq += 1;
                        guard.settlements.push(MockSettlementRecord {
                            acked: false,
                            dto: ContractSettlementDto {
                                settlement_id: format!("mock_set_{seq}"),
                                settlement_type: "OFFER_SOLD".to_string(),
                                target_server_id: seller_sid,
                                target_farm_id: seller_fid,
                                remote_id: rid.to_string(),
                                fill_type: ft,
                                liters: granted,
                                missing_liters: 0,
                                base_trend_factor: 1.0,
                                price_delta_pct: delta,
                                source_label: format!("#{rid}"),
                            },
                        });
                    }
                    Self::store_action_result_locked(
                        guard,
                        &server_id,
                        now,
                        ContractActionResultDto {
                            action_id,
                            sub_action: "BUY_OFFER".to_string(),
                            status: "ACCEPTED".to_string(),
                            reason: String::new(),
                            remote_id: rid.to_string(),
                            granted_liters: granted,
                            base_trend_factor: 1.0,
                            bonus_pct: 0,
                            remaining_seconds: 0,
                        },
                    );
                } else {
                    Self::store_action_result_locked(
                        guard,
                        &server_id,
                        now,
                        ContractActionResultDto {
                            action_id,
                            sub_action: "BUY_OFFER".to_string(),
                            status: "REJECTED".to_string(),
                            reason: "OFFER_UNAVAILABLE".to_string(),
                            remote_id: rid.to_string(),
                            granted_liters: 0,
                            base_trend_factor: 1.0,
                            bonus_pct: 0,
                            remaining_seconds: 0,
                        },
                    );
                }
            }
            ContractSubAction::CancelOffer => {
                let rid = action.remote_id.as_deref().unwrap_or("").trim();
                let farm_id = action.farm_id.unwrap_or(0);
                let mut accepted = false;
                for rec in &mut guard.direct_offers {
                    if rec.dto.remote_id == rid
                        && rec.dto.seller_server_id == server_id
                        && rec.dto.seller_farm_id == farm_id
                        && rec.dto.status == "OPEN"
                        && rec.dto.remaining_liters > 0
                    {
                        rec.dto.status = "CANCELLED".to_string();
                        rec.dto.remaining_liters = 0;
                        accepted = true;
                        break;
                    }
                }
                Self::store_action_result_locked(
                    guard,
                    &server_id,
                    now,
                    ContractActionResultDto {
                        action_id,
                        sub_action: "CANCEL_OFFER".to_string(),
                        status: if accepted { "ACCEPTED" } else { "REJECTED" }.to_string(),
                        reason: if accepted {
                            String::new()
                        } else {
                            "OFFER_ALREADY_SOLD".to_string()
                        },
                        remote_id: rid.to_string(),
                        granted_liters: 0,
                        base_trend_factor: 1.0,
                        bonus_pct: 0,
                        remaining_seconds: 0,
                    },
                );
            }
        }

        Self::contract_snapshot_locked(guard, Some(&server_id))
    }

    fn store_action_result_locked(
        guard: &mut MockState,
        server_id: &str,
        now: i64,
        dto: ContractActionResultDto,
    ) {
        if guard.action_results.len() >= 256 {
            if let Some(oldest_key) = guard
                .action_results
                .iter()
                .min_by_key(|(_, v)| v.created_at)
                .map(|(k, _)| k.clone())
            {
                guard.action_results.remove(&oldest_key);
            }
        }
        guard.action_results.insert(
            dto.action_id.clone(),
            MockActionResultRecord {
                server_id: server_id.to_string(),
                created_at: now,
                dto,
            },
        );
    }
}

/// Linear saturation trend between `0.70` (full market, -30%) and `1.30` (empty market, +30%).
fn calculate_trend(fill_level: i64, capacity: f32) -> f32 {
    let ratio = (fill_level.max(0) as f32 / capacity.max(1.0)).clamp(0.0, 1.0);
    let raw = 1.30 - (ratio * 0.60);
    (raw * 100.0).round() / 100.0
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn unix_now_secs() -> u64 {
    now_unix_secs().max(0) as u64
}
