//! Stage 3 used vehicle market bridge (Spec Section 3.6): `write/vehicleAction_*.xml` ->
//! `POST /api/v1/vehicles/actions`, `GET /api/v1/vehicles/state` -> `read/gmVehicleSync_*.xml`.
//!
//! Mirrors the contract bridge: the same savegame identity (`X-Server-Secret`), a board polled without
//! identity never reaches the mod, and local rejections release the mod's escrow.

use super::{
    count_bridge_queues, emit_snapshot_update, push_i18n_log_shared, push_raw_log_shared,
    record_delivered_action_id, LogLevel, SharedState, SERVER_SECRET_INVALID_REASON,
};
use crate::api::types::{
    ServerSecret, VehicleActionRequest, VehicleActionResultDto, VehicleStateResponse,
    VehicleSubAction,
};
use crate::bridge::paths::BridgePaths;
use crate::bridge::xml::{
    has_unread_vehicle_sync_xml, write_vehicle_data_xml, write_vehicle_data_xml_with_identity,
    write_vehicle_results_xml,
};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

/// Reason passed to the mod when the backend does not offer the vehicle market (any more).
pub(super) const VEHICLES_NOT_SUPPORTED_REASON: &str = "VEHICLES_NOT_SUPPORTED";

/// Unacknowledged settlements are written again after this long. The mod acknowledges only after the
/// game was saved, so open settlements are normal for a whole autosave interval.
const SETTLEMENT_REWRITE_SECS: u64 = 120;

/// Sync state of the vehicle board towards the mod for the current savegame.
#[derive(Default)]
pub(super) struct VehicleBoardSync {
    /// A full authenticated board was written to `read/` at least once.
    pub(super) synced_once: bool,
    /// Last written board (without action results and secret), the baseline for synthetic results.
    last_state: VehicleStateResponse,
    last_write_at: Option<Instant>,
    /// Action results already written, so later polls do not repeat them.
    pub(super) delivered_action_ids: VecDeque<String>,
    announced_settlement_ids: HashSet<String>,
}

impl VehicleBoardSync {
    /// Forgets everything, e.g. when another savegame becomes active.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Last authenticated board, if one was written.
    pub(super) fn baseline(&self) -> Option<&VehicleStateResponse> {
        self.synced_once.then_some(&self.last_state)
    }

    fn secs_since_write(&self) -> u64 {
        self.last_write_at
            .map_or(u64::MAX, |at| at.elapsed().as_secs())
    }
}

/// Writes the authenticated board to `read/gmVehicleSync_*.xml` when needed and updates the counters.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_and_write_vehicle_state(
    paths: &BridgePaths,
    state: &VehicleStateResponse,
    undelivered_identity: Option<(&str, &ServerSecret)>,
    force_write_xml: bool,
    sync: &mut VehicleBoardSync,
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) {
    let elapsed_secs = sync.secs_since_write();
    let settlement_rewrite_due = !state.settlements.is_empty()
        && elapsed_secs >= SETTLEMENT_REWRITE_SECS
        && !has_unread_vehicle_sync_xml(&paths.read_dir);

    let should_write = force_write_xml
        || !sync.synced_once
        || !state.action_results.is_empty()
        || settlement_rewrite_due
        || vehicle_state_semantically_changed(&sync.last_state, state, elapsed_secs);

    if should_write {
        if let Err(err) = write_vehicle_data_xml_with_identity(
            &paths.staging_dir,
            &paths.read_dir,
            state,
            undelivered_identity,
        ) {
            push_raw_log_shared(shared, app_handle, LogLevel::Error, err);
            return;
        }
        for res in &state.action_results {
            record_delivered_action_id(&mut sync.delivered_action_ids, &res.action_id);
        }
        let new_settlements = state
            .settlements
            .iter()
            .filter(|s| sync.announced_settlement_ids.insert(s.settlement_id.clone()))
            .count();
        if state.settlements.is_empty() && sync.announced_settlement_ids.len() > 256 {
            sync.announced_settlement_ids.clear();
        }
        if new_settlements > 0 {
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Success,
                "logVehicleSettlementSynced",
                [("count", new_settlements.to_string())],
                format!(
                    "Vehicle settlements synced: Delivered {new_settlements} vehicle market settlement(s) to FS."
                ),
            );
        }
        sync.last_state = VehicleStateResponse {
            action_results: Vec::new(),
            server_secret: None,
            ..state.clone()
        };
        sync.last_write_at = Some(Instant::now());
        sync.synced_once = true;
    }

    let (r_cnt, w_cnt) = count_bridge_queues(paths);
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    emit_snapshot_update(shared, app_handle, |snap| {
        snap.vehicles_supported = true;
        snap.open_vehicle_listings_count = state.listings.len();
        snap.read_queue_count = r_cnt;
        snap.write_queue_count = w_cnt;
        snap.last_sync_unix = Some(now_secs);
    });
}

/// `true` when the board differs in anything but the countdown (`remainingSeconds` drifts by
/// less than 2 minutes against the elapsed time).
fn vehicle_state_semantically_changed(
    prev: &VehicleStateResponse,
    next: &VehicleStateResponse,
    elapsed_secs: u64,
) -> bool {
    if prev.listings.len() != next.listings.len() || prev.settlements.len() != next.settlements.len()
    {
        return true;
    }
    let elapsed = i64::try_from(elapsed_secs).unwrap_or(i64::MAX);
    let listing_changed = prev.listings.iter().zip(&next.listings).any(|(a, b)| {
        let expected_rem = a.remaining_seconds.saturating_sub(elapsed).max(0);
        let countless = |l: &crate::api::types::OnlineVehicleListingDto| {
            let mut copy = l.clone();
            copy.remaining_seconds = 0;
            copy
        };
        countless(a) != countless(b) || (b.remaining_seconds - expected_rem).abs() > 120
    });
    listing_changed
        || prev
            .settlements
            .iter()
            .zip(&next.settlements)
            .any(|(a, b)| a != b)
}

/// Rejects actions locally so the mod releases its escrow. Without an authenticated board the file
/// carries the results only (no `<listings>`), so the mod keeps its board.
pub(super) fn synthesize_rejected_vehicle_actions(
    paths: &BridgePaths,
    actions: &[VehicleActionRequest],
    reason: &str,
    board_baseline: bool,
    sync: &mut VehicleBoardSync,
) {
    let mut results = Vec::new();
    for act in actions {
        // A rejected identity concerns the whole savegame, so the mod also hears about it for silent actions.
        if reason != SERVER_SECRET_INVALID_REASON && act.sub_action.is_silent() {
            continue;
        }
        record_delivered_action_id(&mut sync.delivered_action_ids, &act.action_id);
        let create_action_id = match act.sub_action {
            VehicleSubAction::CreateListing => act.action_id.clone(),
            _ => act.create_action_id.clone().unwrap_or_default(),
        };
        results.push(VehicleActionResultDto {
            action_id: act.action_id.clone(),
            sub_action: act.sub_action.as_str().to_string(),
            status: "REJECTED".to_string(),
            reason: reason.to_string(),
            remote_id: act.remote_id.clone().unwrap_or_default(),
            create_action_id,
            remaining_seconds: 0,
            vehicle: None,
        });
    }
    if results.is_empty() {
        return;
    }
    let _ = match sync.baseline().filter(|_| board_baseline) {
        Some(baseline) => {
            let synthetic = VehicleStateResponse {
                action_results: results,
                ..baseline.clone()
            };
            write_vehicle_data_xml(&paths.staging_dir, &paths.read_dir, &synthetic)
        }
        None => write_vehicle_results_xml(&paths.staging_dir, &paths.read_dir, &results),
    };
}

/// Short label of a vehicle action for the client log.
pub(super) fn format_vehicle_action_detail(req: &VehicleActionRequest) -> String {
    if let Some(ref vehicle) = req.vehicle {
        let file = vehicle
            .xml_filename
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("")
            .trim_end_matches(".xml");
        let name = match vehicle.mod_name.as_deref() {
            Some(mod_name) if !mod_name.is_empty() => format!("{mod_name}/{file}"),
            _ => file.to_string(),
        };
        return match req.price_factor {
            Some(factor) => format!("{name} • ×{factor:.2}"),
            None => name,
        };
    }
    if let Some(ref rid) = req.remote_id {
        return format!("#{rid}");
    }
    req.create_action_id
        .clone()
        .unwrap_or_else(|| req.action_id.clone())
}

/// Logs the outcome of a user-facing vehicle action (silent actions are not logged).
pub(super) fn log_vehicle_action_outcome(
    req: &VehicleActionRequest,
    rejection: Option<&str>,
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) {
    if req.sub_action.is_silent() {
        return;
    }
    let sub_str = req.sub_action.as_str().to_string();
    match rejection {
        Some(reason) => {
            let reason = if reason.trim().is_empty() {
                "REJECTED".to_string()
            } else {
                reason.trim().to_string()
            };
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Warn,
                "logVehicleActionRejected",
                [("subAction", sub_str.clone()), ("reason", reason.clone())],
                format!("Vehicle action rejected: {sub_str} ({reason})"),
            );
        }
        None => {
            let detail = format_vehicle_action_detail(req);
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Success,
                "logVehicleActionSent",
                [("subAction", sub_str.clone()), ("detail", detail.clone())],
                format!("Vehicle action synced: {sub_str} ({detail})"),
            );
        }
    }
}
