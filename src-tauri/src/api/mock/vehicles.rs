//! Stage 3 used vehicle market of the local Mock backend (Spec Section 3.6, backend API `2.9.0`).
//!
//! Follows the backend rules from `D-S-Agrarservice/GlobalMarketClient#11`: one savegame secret for
//! contracts and vehicles, masked `seller_server_id` / `create_action_id` on other savegames'
//! listings, idempotent actions, the cancel answers (`EXPIRED`, repeated cancel, `NOT_CREATED`) and
//! settlements that stay until `ACK_SETTLEMENT`.

use super::{now_unix_secs, LocalMockBackend, MockState, MOCK_FOREIGN_SERVER_ID};
use crate::api::types::{
    OnlineVehicleListingDto, VehicleActionRequest, VehicleActionResultDto, VehicleConfigDto,
    VehicleDto, VehicleSettlementDto, VehicleStateResponse, VehicleSubAction, MAX_VEHICLE_CONFIGS,
};
use crate::bridge::xml::sanitize_market_name;
use std::collections::{BTreeMap, BTreeSet};

/// Open listings per farm (counted apart from the contract board).
const MAX_OPEN_LISTINGS_PER_FARM: usize = 20;
/// Action results stay in the state this long, like on the backend.
const RESULT_WINDOW_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListingStatus {
    Open,
    Sold,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone)]
struct MockVehicleListing {
    /// Unmasked: `seller_server_id` and `create_action_id` are masked per caller in the snapshot.
    dto: OnlineVehicleListingDto,
    expires_at: i64,
    status: ListingStatus,
}

#[derive(Debug, Clone)]
struct MockVehicleResult {
    created_at: i64,
    dto: VehicleActionResultDto,
}

#[derive(Debug, Clone)]
struct MockVehicleSettlement {
    acked: bool,
    dto: VehicleSettlementDto,
}

/// Vehicle market part of [`MockState`].
#[derive(Debug)]
pub(super) struct MockVehicleMarket {
    pub(super) supported: bool,
    listings: Vec<MockVehicleListing>,
    /// Keyed by `(server_id, action_id)`.
    results: BTreeMap<(String, String), MockVehicleResult>,
    settlements: Vec<MockVehicleSettlement>,
    /// `(server_id, create_action_id)` cancelled before the listing was ever created.
    blocked_creates: BTreeSet<(String, String)>,
    next_listing_seq: u64,
    next_settlement_seq: u64,
}

impl Default for MockVehicleMarket {
    fn default() -> Self {
        let now = now_unix_secs();
        let listings = vec![
            foreign_listing(1, basegame_tractor(), 1.12, now + 259_200),
            foreign_listing(2, modded_trailer_with_paint(), 0.95, now + 86_400),
        ];
        Self {
            supported: true,
            listings,
            results: BTreeMap::new(),
            settlements: Vec::new(),
            blocked_creates: BTreeSet::new(),
            next_listing_seq: 3,
            next_settlement_seq: 1,
        }
    }
}

fn basegame_tractor() -> VehicleDto {
    VehicleDto {
        xml_filename: "data/vehicles/fendt/vario700/vario700.xml".to_string(),
        mod_name: None,
        mod_file_hash: None,
        group: "DRIVABLES".to_string(),
        age_months: 14,
        operating_hours: 230.0,
        damage: 0.05,
        wear: 0.12,
        configurations: vec![VehicleConfigDto {
            name: "motor".to_string(),
            save_id: "2".to_string(),
            is_active: true,
            is_bought: true,
            color: None,
            material_template: None,
        }],
    }
}

fn modded_trailer_with_paint() -> VehicleDto {
    VehicleDto {
        xml_filename: "$moddir$FS25_ExampleTrailer/trailer.xml".to_string(),
        mod_name: Some("FS25_ExampleTrailer".to_string()),
        mod_file_hash: Some("3f1c0d9e8b7a6c5d4e3f2a1b0c9d8e7f".to_string()),
        group: "TRAILERS".to_string(),
        age_months: 26,
        operating_hours: 0.0,
        damage: 0.18,
        wear: 0.31,
        configurations: vec![
            VehicleConfigDto {
                name: "wheel".to_string(),
                save_id: "3".to_string(),
                is_active: true,
                is_bought: true,
                color: None,
                material_template: None,
            },
            VehicleConfigDto {
                name: "baseColor".to_string(),
                save_id: "4".to_string(),
                is_active: true,
                is_bought: true,
                color: Some([0.62, 0.05, 0.04]),
                material_template: Some(String::new()),
            },
        ],
    }
}

fn foreign_listing(seq: u64, vehicle: VehicleDto, price_factor: f64, expires_at: i64) -> MockVehicleListing {
    MockVehicleListing {
        dto: OnlineVehicleListingDto {
            remote_id: format!("mock_v_{seq}"),
            create_action_id: format!("{MOCK_FOREIGN_SERVER_ID}_8000_{seq}"),
            sale_mode: "FIXED".to_string(),
            seller_server_id: MOCK_FOREIGN_SERVER_ID.to_string(),
            seller_farm_id: 1,
            seller_name: "Nordhof [Mock EU]".to_string(),
            price_factor,
            remaining_seconds: 0,
            status: "OPEN".to_string(),
            vehicle,
        },
        expires_at,
        status: ListingStatus::Open,
    }
}

impl MockVehicleMarket {
    fn next_settlement_id(&mut self) -> String {
        let seq = self.next_settlement_seq;
        self.next_settlement_seq += 1;
        format!("vs_mock_{seq}")
    }

    fn push_settlement(&mut self, dto: VehicleSettlementDto) {
        self.settlements.push(MockVehicleSettlement { acked: false, dto });
    }

    /// Expires open listings; the seller gets the vehicle back with `LISTING_EXPIRED`.
    fn process_expirations(&mut self, now: i64) {
        let mut expired = Vec::new();
        for rec in &mut self.listings {
            if rec.status == ListingStatus::Open && rec.expires_at <= now {
                rec.status = ListingStatus::Expired;
                if rec.dto.seller_server_id != MOCK_FOREIGN_SERVER_ID {
                    expired.push(rec.dto.clone());
                }
            }
        }
        for dto in expired {
            let settlement_id = self.next_settlement_id();
            self.push_settlement(VehicleSettlementDto {
                settlement_id,
                settlement_type: "LISTING_EXPIRED".to_string(),
                target_server_id: dto.seller_server_id,
                target_farm_id: dto.seller_farm_id,
                remote_id: dto.remote_id,
                create_action_id: dto.create_action_id,
                buy_action_id: String::new(),
                price_factor: dto.price_factor,
                buyer_name: String::new(),
                seller_name: String::new(),
                vehicle: Some(dto.vehicle),
            });
        }
    }

    pub(super) fn snapshot(&mut self, server_id: Option<&str>) -> VehicleStateResponse {
        let now = now_unix_secs();
        self.process_expirations(now);
        let sid = server_id.map(str::trim).filter(|s| !s.is_empty());
        let is_own = |id: &str| sid.is_some_and(|s| s == id);

        let listings = self
            .listings
            .iter()
            .filter(|rec| rec.status == ListingStatus::Open)
            .map(|rec| {
                let mut dto = rec.dto.clone();
                dto.remaining_seconds = (rec.expires_at - now).max(0);
                // Like the backend: other savegames' ids are never sent (the create id contains it).
                if !is_own(&dto.seller_server_id) {
                    dto.seller_server_id.clear();
                    dto.create_action_id.clear();
                }
                dto
            })
            .collect();

        let mut action_results: Vec<_> = match sid {
            Some(s) => self
                .results
                .iter()
                .filter(|((owner, _), r)| owner == s && now - r.created_at <= RESULT_WINDOW_SECS)
                .map(|(_, r)| r.clone())
                .collect(),
            None => Vec::new(),
        };
        action_results.sort_by_key(|r| r.created_at);

        let settlements = match sid {
            Some(s) => self
                .settlements
                .iter()
                .filter(|rec| !rec.acked && rec.dto.target_server_id == s)
                .map(|rec| rec.dto.clone())
                .collect(),
            None => Vec::new(),
        };

        VehicleStateResponse {
            updated_at: now,
            listings,
            action_results: action_results.into_iter().map(|r| r.dto).collect(),
            settlements,
            server_secret: None,
        }
    }

    fn store_result(&mut self, server_id: &str, now: i64, dto: VehicleActionResultDto) {
        if self.results.len() >= 256 {
            if let Some(oldest) = self
                .results
                .iter()
                .min_by_key(|(_, r)| r.created_at)
                .map(|(k, _)| k.clone())
            {
                self.results.remove(&oldest);
            }
        }
        self.results.insert(
            (server_id.to_string(), dto.action_id.clone()),
            MockVehicleResult { created_at: now, dto },
        );
    }

    pub(super) fn apply_action(&mut self, now: i64, action: VehicleActionRequest) -> VehicleStateResponse {
        self.process_expirations(now);
        let action_id = action.action_id.trim().to_string();
        let server_id = action.server_id.trim().to_ascii_lowercase();

        // Idempotent replay: the stored result is returned again, nothing changes twice.
        if self.results.contains_key(&(server_id.clone(), action_id.clone())) {
            return self.snapshot(Some(&server_id));
        }

        let result = |status: &str, reason: &str| VehicleActionResultDto {
            action_id: action_id.clone(),
            sub_action: action.sub_action.as_str().to_string(),
            status: status.to_string(),
            reason: reason.to_string(),
            remote_id: action.remote_id.clone().unwrap_or_default(),
            create_action_id: String::new(),
            remaining_seconds: 0,
            vehicle: None,
        };

        let dto = match action.sub_action {
            VehicleSubAction::RequestSync => return self.snapshot(Some(&server_id)),
            VehicleSubAction::AckSettlement => {
                let ids = action.settlement_ids.clone().unwrap_or_default();
                for id in ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    for rec in &mut self.settlements {
                        if rec.dto.settlement_id == id && rec.dto.target_server_id == server_id {
                            rec.acked = true;
                        }
                    }
                }
                return self.snapshot(Some(&server_id));
            }
            VehicleSubAction::CreateListing => self.create_listing(now, &server_id, &action, result),
            VehicleSubAction::CancelListing => self.cancel_listing(now, &server_id, &action, result),
            VehicleSubAction::BuyListing => self.buy_listing(&server_id, &action, result),
        };
        self.store_result(&server_id, now, dto);
        self.snapshot(Some(&server_id))
    }

    fn create_listing(
        &mut self,
        now: i64,
        server_id: &str,
        action: &VehicleActionRequest,
        result: impl Fn(&str, &str) -> VehicleActionResultDto,
    ) -> VehicleActionResultDto {
        let action_id = action.action_id.trim().to_string();
        let rejected = |reason: &str| VehicleActionResultDto {
            create_action_id: action_id.clone(),
            ..result("REJECTED", reason)
        };
        if self
            .blocked_creates
            .contains(&(server_id.to_string(), action_id.clone()))
        {
            return rejected("NOT_CREATED");
        }
        let farm_id = action.farm_id.unwrap_or(0);
        let price_factor = action.price_factor.unwrap_or(0.0);
        let hours = action.duration_hours.unwrap_or(0);
        let sale_mode = action
            .sale_mode
            .as_deref()
            .map(|s| s.trim().to_ascii_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "FIXED".to_string());
        let vehicle = match action.vehicle.clone() {
            Some(v)
                if !v.xml_filename.trim().is_empty()
                    && (0.0..=1.0).contains(&v.damage)
                    && (0.0..=1.0).contains(&v.wear)
                    && v.configurations.len() <= MAX_VEHICLE_CONFIGS =>
            {
                v
            }
            _ => return rejected("INVALID_REQUEST"),
        };
        if farm_id <= 0
            || !(0.70..=1.30).contains(&price_factor)
            || !(1..=168).contains(&hours)
            || sale_mode != "FIXED"
        {
            return rejected("INVALID_REQUEST");
        }
        let open_of_farm = self
            .listings
            .iter()
            .filter(|l| {
                l.status == ListingStatus::Open
                    && l.dto.seller_server_id == server_id
                    && l.dto.seller_farm_id == farm_id
            })
            .count();
        if open_of_farm >= MAX_OPEN_LISTINGS_PER_FARM {
            return rejected("LIMIT_REACHED");
        }

        let seq = self.next_listing_seq;
        self.next_listing_seq += 1;
        let remote_id = format!("mock_v_{seq}");
        let secs = i64::from(hours) * 3_600;
        let seller_name = action
            .farm_name
            .as_deref()
            .and_then(sanitize_market_name)
            .unwrap_or_else(|| format!("Farm #{farm_id}"));
        self.listings.push(MockVehicleListing {
            dto: OnlineVehicleListingDto {
                remote_id: remote_id.clone(),
                create_action_id: action_id.clone(),
                sale_mode,
                seller_server_id: server_id.to_string(),
                seller_farm_id: farm_id,
                seller_name,
                price_factor,
                remaining_seconds: secs,
                status: "OPEN".to_string(),
                vehicle,
            },
            expires_at: now + secs,
            status: ListingStatus::Open,
        });
        VehicleActionResultDto {
            remote_id,
            create_action_id: action_id,
            remaining_seconds: secs,
            ..result("ACCEPTED", "")
        }
    }

    fn cancel_listing(
        &mut self,
        now: i64,
        server_id: &str,
        action: &VehicleActionRequest,
        result: impl Fn(&str, &str) -> VehicleActionResultDto,
    ) -> VehicleActionResultDto {
        let remote_id = action.remote_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let create_id = action
            .create_action_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let found = match (remote_id, create_id) {
            (Some(rid), _) => self.listings.iter().position(|l| l.dto.remote_id == rid),
            (None, Some(cid)) => self
                .listings
                .iter()
                .position(|l| l.dto.seller_server_id == server_id && l.dto.create_action_id == cid),
            (None, None) => return result("REJECTED", "INVALID_REQUEST"),
        };

        let Some(idx) = found else {
            return match create_id.filter(|_| remote_id.is_none()) {
                // Never created (or its create was rejected): block the id for good.
                Some(cid) => {
                    self.blocked_creates
                        .insert((server_id.to_string(), cid.to_string()));
                    VehicleActionResultDto {
                        create_action_id: cid.to_string(),
                        ..result("ACCEPTED", "NOT_CREATED")
                    }
                }
                None => result("REJECTED", "LISTING_NOT_FOUND"),
            };
        };

        let rec = &mut self.listings[idx];
        let answer = VehicleActionResultDto {
            remote_id: rec.dto.remote_id.clone(),
            create_action_id: rec.dto.create_action_id.clone(),
            ..result("REJECTED", "")
        };
        if rec.dto.seller_server_id != server_id
            || action.farm_id.is_some_and(|f| f != rec.dto.seller_farm_id)
        {
            return VehicleActionResultDto {
                reason: "NOT_OWNER".to_string(),
                create_action_id: String::new(),
                ..answer
            };
        }
        match rec.status {
            ListingStatus::Sold => VehicleActionResultDto {
                reason: "LISTING_SOLD".to_string(),
                ..answer
            },
            ListingStatus::Expired => VehicleActionResultDto {
                reason: "EXPIRED".to_string(),
                ..answer
            },
            // A repeated cancel answers again with the vehicle: the first answer may have been lost.
            ListingStatus::Open | ListingStatus::Cancelled => {
                rec.status = ListingStatus::Cancelled;
                rec.expires_at = rec.expires_at.min(now);
                VehicleActionResultDto {
                    status: "ACCEPTED".to_string(),
                    vehicle: Some(rec.dto.vehicle.clone()),
                    ..answer
                }
            }
        }
    }

    fn buy_listing(
        &mut self,
        server_id: &str,
        action: &VehicleActionRequest,
        result: impl Fn(&str, &str) -> VehicleActionResultDto,
    ) -> VehicleActionResultDto {
        let farm_id = action.farm_id.unwrap_or(0);
        let Some(rid) = action.remote_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            return result("REJECTED", "INVALID_REQUEST");
        };
        if farm_id <= 0 {
            return result("REJECTED", "INVALID_REQUEST");
        }
        let Some(idx) = self.listings.iter().position(|l| l.dto.remote_id == rid) else {
            return result("REJECTED", "LISTING_NOT_FOUND");
        };
        let rec = &self.listings[idx];
        let reason = match rec.status {
            ListingStatus::Sold => Some("LISTING_SOLD"),
            ListingStatus::Expired => Some("EXPIRED"),
            ListingStatus::Cancelled => Some("LISTING_NOT_FOUND"),
            ListingStatus::Open
                if rec.dto.seller_server_id == server_id && rec.dto.seller_farm_id == farm_id =>
            {
                Some("OWN_LISTING")
            }
            ListingStatus::Open => None,
        };
        if let Some(reason) = reason {
            return result("REJECTED", reason);
        }

        self.listings[idx].status = ListingStatus::Sold;
        let listing = self.listings[idx].dto.clone();
        let buyer_name = action
            .farm_name
            .as_deref()
            .and_then(sanitize_market_name)
            .unwrap_or_else(|| format!("Farm #{farm_id}"));
        let bought_id = self.next_settlement_id();
        self.push_settlement(VehicleSettlementDto {
            settlement_id: bought_id,
            settlement_type: "LISTING_BOUGHT".to_string(),
            target_server_id: server_id.to_string(),
            target_farm_id: farm_id,
            remote_id: listing.remote_id.clone(),
            create_action_id: String::new(),
            buy_action_id: action.action_id.trim().to_string(),
            price_factor: listing.price_factor,
            buyer_name: String::new(),
            seller_name: listing.seller_name.clone(),
            vehicle: Some(listing.vehicle.clone()),
        });
        if listing.seller_server_id != MOCK_FOREIGN_SERVER_ID {
            self.push_sold_settlement(&listing, &buyer_name);
        }
        VehicleActionResultDto {
            vehicle: Some(listing.vehicle),
            ..result("ACCEPTED", "")
        }
    }

    fn push_sold_settlement(&mut self, listing: &OnlineVehicleListingDto, buyer_name: &str) {
        let settlement_id = self.next_settlement_id();
        self.push_settlement(VehicleSettlementDto {
            settlement_id,
            settlement_type: "LISTING_SOLD".to_string(),
            target_server_id: listing.seller_server_id.clone(),
            target_farm_id: listing.seller_farm_id,
            remote_id: listing.remote_id.clone(),
            create_action_id: listing.create_action_id.clone(),
            buy_action_id: String::new(),
            price_factor: listing.price_factor,
            buyer_name: buyer_name.to_string(),
            seller_name: String::new(),
            vehicle: None,
        });
    }

    /// A foreign farm buys every own open listing (`LISTING_SOLD`) and a fresh foreign listing appears.
    fn simulate_activity(&mut self, now: i64) -> String {
        self.process_expirations(now);
        let own_open: Vec<usize> = self
            .listings
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.status == ListingStatus::Open && l.dto.seller_server_id != MOCK_FOREIGN_SERVER_ID
            })
            .map(|(i, _)| i)
            .collect();
        for &idx in &own_open {
            self.listings[idx].status = ListingStatus::Sold;
            let listing = self.listings[idx].dto.clone();
            self.push_sold_settlement(&listing, "Hof Nordwind [Mock]");
        }
        let seq = self.next_listing_seq;
        self.next_listing_seq += 1;
        let vehicle = if seq % 2 == 0 {
            modded_trailer_with_paint()
        } else {
            basegame_tractor()
        };
        self.listings
            .push(foreign_listing(seq, vehicle, 1.05, now + 172_800));
        format!("vehicles_sold={}, new_vehicle_listings=1", own_open.len())
    }
}

impl LocalMockBackend {
    /// Enables or disables `vehicles_supported` in Mock mode.
    pub fn set_simulated_vehicles_supported(&self, supported: bool) {
        if let Ok(mut guard) = self.state.lock() {
            guard.vehicles.supported = supported;
        }
    }

    /// Lets every open vehicle listing expire now (`LISTING_EXPIRED` for own listings).
    pub fn expire_all_vehicle_listings(&self) {
        if let Ok(mut guard) = self.state.lock() {
            let past = now_unix_secs() - 1;
            for rec in &mut guard.vehicles.listings {
                if rec.status == ListingStatus::Open {
                    rec.expires_at = past;
                }
            }
            guard.vehicles.process_expirations(past + 1);
        }
    }

    /// Simulates foreign buyers and sellers on the vehicle market in Developer Mock mode.
    pub fn simulate_vehicle_activity(&self) -> String {
        match self.state.lock() {
            Ok(mut guard) => guard.vehicles.simulate_activity(now_unix_secs()),
            Err(_) => "mutex poisoned".to_string(),
        }
    }

    pub(super) fn vehicles_snapshot_locked(
        guard: &mut MockState,
        server_id: Option<&str>,
    ) -> VehicleStateResponse {
        guard.vehicles.snapshot(server_id)
    }
}
