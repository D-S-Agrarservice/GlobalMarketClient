//! Local in-memory simulation backend (`LocalMockBackend`).
//!
//! Allows testing 100% of the FS25 Lua mod online features locally in-game
//! (including `APPROVED` / `PENDING` / `REJECTED` moderation, authoritative
//! `greatDemand` triggers, buy/sell stock deltas, and `minVersion` force-update warnings)
//! without requiring a live FastAPI server.

use crate::api::types::{
    FillTypeStatus, MarketItemDto, MarketStateResponse, ServerHandshake, TransactionRequest,
    TransactionType, WhitelistSyncRequest,
};
use crate::api::{ApiError, BackendService};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_CAPACITY_LITERS: f32 = 2_000_000.0;

#[derive(Debug)]
struct MockState {
    simulate_offline: bool,
    simulated_min_version: String,
    simulated_logo_url: Option<String>,
    items: BTreeMap<String, MarketItemDto>,
    processed_tx_ids: Vec<String>,
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

        Self {
            simulate_offline: false,
            simulated_min_version: "1.0.0".to_string(),
            simulated_logo_url: None,
            items,
            processed_tx_ids: Vec::new(),
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

    /// Set or clear a simulated Community Logo (`data:image/...;base64,...` or URL) in Mock mode.
    pub fn set_simulated_logo(&self, logo_url: Option<String>) {
        if let Ok(mut guard) = self.state.lock() {
            guard.simulated_logo_url = logo_url;
        }
    }

    /// Returns the currently active simulated Community Logo URL/data-URI, if any.
    pub fn simulated_logo(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|guard| guard.simulated_logo_url.clone())
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

        Ok(ServerHandshake {
            online: true,
            min_exe_version: guard.simulated_min_version.clone(),
            poll_interval_sec: 5,
            market_name: "Local Simulation (Mock)".to_string(),
            api_version: "2.0.0-mock".to_string(),
            market_logo_url: guard.simulated_logo_url.clone(),
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
