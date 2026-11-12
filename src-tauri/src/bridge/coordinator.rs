//! Central background bridge coordinator:
//! Combines `notify` event watching + `500 ms` OneDrive-safe fallback polling,
//! `3.5 s` `globalMarketOnline.xml` heartbeats, periodic backend state sync,
//! Great Demand transition detection, structured multi-language log events,
//! and instant shutdown cleanup.

use crate::api::fastapi::FastApiBackend;
use crate::api::mock::LocalMockBackend;
use crate::api::types::{FillTypeStatus, MarketItemDto};
use crate::api::{ApiError, BackendService};
use crate::bridge::l10n::localize_fill_type;
use crate::bridge::paths::{discover_fs25_profile_path, resolve_bridge_paths, BridgePaths};
use crate::bridge::xml::{
    cleanup_bridge_runtime_files, is_version_sufficient, parse_incoming_write_xml,
    prune_market_data_xmls, write_heartbeat_xml, write_market_data_xml, IncomingPacket,
};
use crate::config::{
    qualify_transaction_id, verify_access_key, AccessTier, AppConfig, BackendMode,
    DEFAULT_OFFICIAL_BACKEND_URL,
};
use crate::PROTOCOL_EXE_VERSION;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const MAX_LOG_ENTRIES: usize = 250;
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(3500);
const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_millis(500);
/// Support/Diagnostic (`AccessTier::Extended`) mode automatically resets to `User` after 30 minutes.
pub const EXTENDED_TIER_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BridgeConnectionState {
    PathMissing,
    Connecting,
    Online,
    BackendOffline,
    Outdated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub timestamp_ms: u64,
    pub level: LogLevel,
    /// English fallback text (used directly in diagnostic reports and if no i18n key matches).
    pub message: String,
    /// Optional frontend translation key (`src/locales/*.json`) so logs switch languages dynamically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Named interpolation parameters for `key`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GreatDemandNotification {
    pub fill_type: String,
    pub title: String,
    pub bonus_percent: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatTickPayload {
    pub seq: u64,
    pub timestamp_unix: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSnapshot {
    pub state: BridgeConnectionState,
    /// Dynamically detected Farming Simulator edition (e.g. `"FS25"`, `"FS28"`), or `None` before selection.
    pub game_edition: Option<String>,
    pub access_tier: AccessTier,
    pub backend_mode: BackendMode,
    pub backend_url: String,
    pub api_version: Option<String>,
    pub market_name: String,
    pub market_logo_url: Option<String>,
    pub profile_path: Option<String>,
    pub bridge_path: Option<String>,
    pub exe_version: String,
    pub min_exe_version: String,
    pub approved_count: usize,
    pub pending_count: usize,
    pub rejected_count: usize,
    pub read_queue_count: usize,
    pub write_queue_count: usize,
    pub active_great_demands: Vec<String>,
    pub active_great_demand_codes: Vec<String>,
    pub heartbeat_seq: u64,
    pub last_heartbeat_unix: Option<u64>,
    pub last_sync_unix: Option<u64>,
    pub last_error: Option<String>,
}

enum WorkerSignal {
    Wakeup,
    ForceSyncNow,
    RefreshLocalization,
    Stop,
}

struct SharedState {
    config: AppConfig,
    config_file: PathBuf,
    snapshot: BridgeSnapshot,
    logs: VecDeque<LogEntry>,
    extended_expires_at: Option<Instant>,
}

pub struct BridgeManager {
    app_handle: Option<AppHandle>,
    shared: Arc<Mutex<SharedState>>,
    mock_backend: Arc<LocalMockBackend>,
    live_backend: Arc<FastApiBackend>,
    worker_tx: Mutex<Option<Sender<WorkerSignal>>>,
    worker_stop_flag: Mutex<Option<Arc<AtomicBool>>>,
    worker_handle: Mutex<Option<JoinHandle<()>>>,
    active_paths: Mutex<Option<BridgePaths>>,
}

impl BridgeManager {
    pub fn new(config_file: PathBuf, app_handle: Option<AppHandle>) -> Self {
        let config_existed = config_file.is_file();
        let mut config = AppConfig::load_from_file(&config_file);
        let mut should_persist = !config_existed;

        // Auto-discover FS25 profile path on first start if none is saved yet.
        if config.fs25_profile_path.is_none() {
            if let Some(found) = discover_fs25_profile_path() {
                config.fs25_profile_path = Some(found.to_string_lossy().into_owned());
                should_persist = true;
            }
        }

        if should_persist {
            let _ = config.save_to_file(&config_file);
        }

        let live_backend = Arc::new(
            FastApiBackend::new(
                config.backend_url.clone(),
                config.market_token.clone(),
                config.client_id.clone(),
            )
            .expect("HTTP client initialization failed"),
        );
        let mock_backend = Arc::new(LocalMockBackend::new());

        let initial_snapshot = BridgeSnapshot {
            state: BridgeConnectionState::PathMissing,
            game_edition: None,
            access_tier: config.access_tier,
            backend_mode: config.backend_mode,
            backend_url: config.backend_url.clone(),
            api_version: None,
            market_name: match config.backend_mode {
                BackendMode::Mock => "Local Simulation (Mock)".to_string(),
                BackendMode::Live => "Global Market".to_string(),
            },
            market_logo_url: None,
            profile_path: config.fs25_profile_path.clone(),
            bridge_path: None,
            exe_version: PROTOCOL_EXE_VERSION.to_string(),
            min_exe_version: "1.0.0".to_string(),
            approved_count: 0,
            pending_count: 0,
            rejected_count: 0,
            read_queue_count: 0,
            write_queue_count: 0,
            active_great_demands: Vec::new(),
            active_great_demand_codes: Vec::new(),
            heartbeat_seq: 0,
            last_heartbeat_unix: None,
            last_sync_unix: None,
            last_error: None,
        };

        let mgr = Self {
            app_handle,
            shared: Arc::new(Mutex::new(SharedState {
                config,
                config_file,
                snapshot: initial_snapshot,
                logs: VecDeque::with_capacity(MAX_LOG_ENTRIES),
                extended_expires_at: None,
            })),
            mock_backend,
            live_backend,
            worker_tx: Mutex::new(None),
            worker_stop_flag: Mutex::new(None),
            worker_handle: Mutex::new(None),
            active_paths: Mutex::new(None),
        };

        mgr.restart_worker();
        mgr
    }

    pub fn get_snapshot(&self) -> BridgeSnapshot {
        check_and_expire_extended_tier_shared(&self.shared, &self.app_handle);
        self.shared
            .lock()
            .map(|s| s.snapshot.clone())
            .unwrap_or_else(|e| e.into_inner().snapshot.clone())
    }

    pub fn get_logs(&self) -> Vec<LogEntry> {
        self.shared
            .lock()
            .map(|s| s.logs.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get_config(&self) -> AppConfig {
        check_and_expire_extended_tier_shared(&self.shared, &self.app_handle);
        self.shared
            .lock()
            .map(|s| s.config.clone())
            .unwrap_or_default()
    }

    /// Sets and validates a manually chosen Farming Simulator folder, persists settings, and restarts the bridge worker.
    pub fn set_profile_path(&self, raw_path: &str) -> Result<BridgeSnapshot, String> {
        let resolved = resolve_bridge_paths(Path::new(raw_path.trim()))?;
        resolved.ensure_directories()?;

        // Store the clean profile root unless re-resolving it would point to a different edition/bridge_root.
        let persisted_dir = match resolve_bridge_paths(&resolved.profile_dir) {
            Ok(recheck) if recheck.bridge_root == resolved.bridge_root => &resolved.profile_dir,
            _ => &resolved.bridge_root,
        };

        {
            let mut guard = self
                .shared
                .lock()
                .map_err(|_| "Internal state lock poisoned".to_string())?;
            guard.config.fs25_profile_path =
                Some(persisted_dir.to_string_lossy().into_owned());
            guard.config.save_to_file(&guard.config_file)?;
        }

        let path_str = persisted_dir.display().to_string();
        self.push_i18n_log(
            LogLevel::Success,
            "logProfilePathSet",
            [
                ("path", path_str.clone()),
                ("edition", resolved.game_edition.clone()),
            ],
            format!(
                "{} directory set: {path_str}",
                resolved.game_edition
            ),
        );
        self.restart_worker();
        Ok(self.get_snapshot())
    }

    /// Updates user configuration (language, notifications, tray behavior, backend mode/URL/token).
    pub fn update_config(&self, mut next: AppConfig) -> Result<BridgeSnapshot, String> {
        next.language = crate::config::normalize_language(&next.language);

        let (needs_worker_restart, lang_changed) = {
            let mut guard = self
                .shared
                .lock()
                .map_err(|_| "Internal state lock poisoned".to_string())?;

            // Preserve immutable fields controlled exclusively by Rust.
            next.client_id = guard.config.client_id.clone();
            next.access_tier = guard.config.access_tier;

            // Only Developer mode is permitted to switch into Mock simulation or override Backend URL.
            if next.access_tier != AccessTier::Developer {
                next.backend_mode = BackendMode::Live;
                next.backend_url = DEFAULT_OFFICIAL_BACKEND_URL.to_string();
            }

            let lang_diff = guard.config.language != next.language;
            let restart = guard.config.backend_mode != next.backend_mode
                || guard.config.backend_url != next.backend_url
                || guard.config.market_token != next.market_token
                || guard.config.fs25_profile_path != next.fs25_profile_path;

            self.live_backend.update_connection_config(
                next.backend_url.clone(),
                next.market_token.clone(),
                next.client_id.clone(),
            );

            guard.snapshot.access_tier = next.access_tier;
            guard.snapshot.backend_mode = next.backend_mode;
            guard.snapshot.backend_url = next.backend_url.clone();

            guard.config = next;
            guard.config.save_to_file(&guard.config_file)?;
            (restart, lang_diff)
        };

        if needs_worker_restart {
            self.restart_worker();
        } else if lang_changed {
            self.nudge_signal(WorkerSignal::RefreshLocalization);
        }
        Ok(self.get_snapshot())
    }

    /// Validates an access key and unlocks `AccessTier::Extended` (30-minute session TTL)
    /// or `AccessTier::Developer` (persistent across restarts).
    pub fn unlock_access_tier(&self, raw_key: &str) -> Result<AppConfig, String> {
        self.unlock_access_tier_with_ttl(raw_key, EXTENDED_TIER_TIMEOUT)
    }

    /// Internal/testable variant allowing a custom TTL for `AccessTier::Extended`.
    pub fn unlock_access_tier_with_ttl(
        &self,
        raw_key: &str,
        extended_ttl: Duration,
    ) -> Result<AppConfig, String> {
        let Some(tier) = verify_access_key(raw_key) else {
            return Err("INVALID_ACCESS_KEY".to_string());
        };
        self.apply_verified_access_tier(tier, extended_ttl)
    }

    /// Testable variant that verifies `raw_key` against explicit target digests (so integration
    /// tests never embed production access keys in plaintext) and applies the unlocked tier.
    pub fn unlock_access_tier_with_digests(
        &self,
        raw_key: &str,
        extended_digest: (u64, u64),
        developer_digest: (u64, u64),
        extended_ttl: Duration,
    ) -> Result<AppConfig, String> {
        let Some(tier) =
            crate::config::verify_access_key_with_digests(raw_key, extended_digest, developer_digest)
        else {
            return Err("INVALID_ACCESS_KEY".to_string());
        };
        self.apply_verified_access_tier(tier, extended_ttl)
    }

    fn apply_verified_access_tier(
        &self,
        tier: AccessTier,
        extended_ttl: Duration,
    ) -> Result<AppConfig, String> {

        let (updated_cfg, needs_restart) = {
            let mut guard = self
                .shared
                .lock()
                .map_err(|_| "Internal state lock poisoned".to_string())?;

            let mut restart = false;
            guard.config.access_tier = tier;
            guard.extended_expires_at = match tier {
                AccessTier::Extended => Some(Instant::now() + extended_ttl),
                AccessTier::User | AccessTier::Developer => None,
            };

            if tier != AccessTier::Developer {
                if guard.config.backend_mode != BackendMode::Live
                    || guard.config.backend_url != DEFAULT_OFFICIAL_BACKEND_URL
                {
                    guard.config.backend_mode = BackendMode::Live;
                    guard.config.backend_url = DEFAULT_OFFICIAL_BACKEND_URL.to_string();
                    self.live_backend.update_connection_config(
                        guard.config.backend_url.clone(),
                        guard.config.market_token.clone(),
                        guard.config.client_id.clone(),
                    );
                    restart = true;
                }
            }

            guard.snapshot.access_tier = tier;
            guard.snapshot.backend_mode = guard.config.backend_mode;
            guard.snapshot.backend_url = guard.config.backend_url.clone();
            guard.config.save_to_file(&guard.config_file)?;
            (guard.config.clone(), restart)
        };

        self.push_i18n_log(
            LogLevel::Info,
            "logAccessTierChanged",
            [("tier", tier.as_str().to_string())],
            format!("Access mode changed to '{}'.", tier.as_str()),
        );

        if needs_restart {
            self.restart_worker();
        } else {
            self.emit_current_snapshot();
        }
        Ok(updated_cfg)
    }

    /// Locks Extended/Developer controls and restores standard player settings (`AccessTier::User`, `Live` mode).
    pub fn reset_access_tier(&self) -> Result<AppConfig, String> {
        let (updated_cfg, needs_restart) = {
            let mut guard = self
                .shared
                .lock()
                .map_err(|_| "Internal state lock poisoned".to_string())?;

            let restart = guard.config.backend_mode != BackendMode::Live
                || guard.config.backend_url != DEFAULT_OFFICIAL_BACKEND_URL;

            guard.extended_expires_at = None;
            guard.config.access_tier = AccessTier::User;
            guard.config.backend_mode = BackendMode::Live;
            guard.config.backend_url = DEFAULT_OFFICIAL_BACKEND_URL.to_string();
            self.live_backend.update_connection_config(
                guard.config.backend_url.clone(),
                guard.config.market_token.clone(),
                guard.config.client_id.clone(),
            );

            guard.snapshot.access_tier = AccessTier::User;
            guard.snapshot.backend_mode = BackendMode::Live;
            guard.snapshot.backend_url = guard.config.backend_url.clone();
            guard.config.save_to_file(&guard.config_file)?;
            (guard.config.clone(), restart)
        };

        self.push_i18n_log(
            LogLevel::Info,
            "logAccessTierChanged",
            [("tier", "user".to_string())],
            "Access mode reset to 'user'.".to_string(),
        );

        if needs_restart {
            self.restart_worker();
        } else {
            self.emit_current_snapshot();
        }
        Ok(updated_cfg)
    }

    pub fn mock_trigger_great_demand(&self, fill_type: Option<String>) -> Option<String> {
        if self.get_config().access_tier != AccessTier::Developer {
            return None;
        }
        let triggered = self
            .mock_backend
            .trigger_great_demand(fill_type.as_deref());
        if let Some(ref ft) = triggered {
            self.push_i18n_log(
                LogLevel::Info,
                "logMockGreatDemand",
                [("item", ft.clone()), ("itemCode", ft.clone())],
                format!("Simulation: Triggered Great Demand for '{ft}' (+25%, 200,000 L)."),
            );
            self.nudge_signal(WorkerSignal::ForceSyncNow);
        }
        triggered
    }

    pub fn mock_approve_pending(&self) -> usize {
        if self.get_config().access_tier != AccessTier::Developer {
            return 0;
        }
        let count = self.mock_backend.approve_all_pending();
        self.push_i18n_log(
            LogLevel::Success,
            "logMockApproved",
            [("count", count.to_string())],
            format!("Simulation: Approved {count} pending mod items (PENDING -> APPROVED)."),
        );
        self.nudge_signal(WorkerSignal::ForceSyncNow);
        count
    }

    pub fn mock_set_offline(&self, offline: bool) {
        if self.get_config().access_tier != AccessTier::Developer {
            return;
        }
        self.mock_backend.set_simulated_offline(offline);
        if offline {
            self.push_i18n_log(
                LogLevel::Warn,
                "logMockOfflineOn",
                [],
                "Simulation: Server outage enabled (isConnected=false).".to_string(),
            );
        } else {
            self.push_i18n_log(
                LogLevel::Info,
                "logMockOfflineOff",
                [],
                "Simulation: Server connection restored.".to_string(),
            );
        }
        self.nudge_signal(WorkerSignal::ForceSyncNow);
    }

    pub fn mock_set_min_version(&self, min_version: String) {
        if self.get_config().access_tier != AccessTier::Developer {
            return;
        }
        self.mock_backend
            .set_simulated_min_version(min_version.clone());
        self.push_i18n_log(
            LogLevel::Info,
            "logMockMinVersion",
            [("minVersion", min_version.clone())],
            format!("Simulation: Minimum required version set to v{min_version}."),
        );
        self.nudge_signal(WorkerSignal::ForceSyncNow);
    }

    /// Loads (or clears when `None`) a local `.png` / `.webp` / `.jpg` / `.dds` image as a simulated
    /// Community Market logo in Developer Mock mode.
    pub fn mock_set_community_logo(
        &self,
        raw_path: Option<String>,
    ) -> Result<BridgeSnapshot, String> {
        let cfg = self.get_config();
        if cfg.access_tier != AccessTier::Developer {
            return Err("Developer mode required.".to_string());
        }

        let cleaned = raw_path
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());

        match cleaned {
            None => {
                self.mock_backend.set_simulated_logo(None);
                if cfg.backend_mode == BackendMode::Mock {
                    self.update_snapshot(|snap| {
                        snap.market_logo_url = None;
                    });
                }
                self.push_i18n_log(
                    LogLevel::Info,
                    "logMockLogoReset",
                    [],
                    "Simulation: Community logo reset to default DSA logo.".to_string(),
                );
            }
            Some(path_str) => {
                let file_path = Path::new(&path_str);
                let data_url = load_local_logo_as_data_url(file_path)?;
                let file_label = file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("logo")
                    .to_string();

                self.mock_backend.set_simulated_logo(Some(data_url.clone()));
                if cfg.backend_mode == BackendMode::Mock {
                    self.update_snapshot(|snap| {
                        snap.market_logo_url = Some(data_url);
                    });
                }
                self.push_i18n_log(
                    LogLevel::Success,
                    "logMockLogoSet",
                    [("file", file_label.clone())],
                    format!("Simulation: Loaded community logo preview '{file_label}'."),
                );
            }
        }

        self.nudge_signal(WorkerSignal::ForceSyncNow);
        Ok(self.get_snapshot())
    }

    /// Signals the worker to stop immediately, cleans up `globalMarketOnline.xml` + `read/gmDataSync_*.xml`
    /// synchronously, and returns the worker `JoinHandle` so GUI shutdown never blocks the main event loop.
    pub fn prepare_fast_exit(&self) -> Option<JoinHandle<()>> {
        if let Ok(mut flag_guard) = self.worker_stop_flag.lock() {
            if let Some(flag) = flag_guard.take() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        if let Ok(mut tx_guard) = self.worker_tx.lock() {
            if let Some(tx) = tx_guard.take() {
                let _ = tx.send(WorkerSignal::Stop);
            }
        }
        if let Ok(mut path_guard) = self.active_paths.lock() {
            if let Some(paths) = path_guard.take() {
                cleanup_bridge_runtime_files(
                    &paths.online_xml_file,
                    &paths.read_dir,
                    &paths.staging_dir,
                );
            }
        }
        self.worker_handle
            .lock()
            .ok()
            .and_then(|mut handle_guard| handle_guard.take())
    }

    /// Cleanly stops the worker and removes `globalMarketOnline.xml` + unread `read/gmDataSync_*.xml` files.
    pub fn shutdown(&self) {
        if let Ok(mut flag_guard) = self.worker_stop_flag.lock() {
            if let Some(flag) = flag_guard.take() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        if let Ok(mut tx_guard) = self.worker_tx.lock() {
            if let Some(tx) = tx_guard.take() {
                let _ = tx.send(WorkerSignal::Stop);
            }
        }
        if let Ok(mut path_guard) = self.active_paths.lock() {
            if let Some(paths) = path_guard.take() {
                cleanup_bridge_runtime_files(
                    &paths.online_xml_file,
                    &paths.read_dir,
                    &paths.staging_dir,
                );
            }
        }
        if let Ok(mut handle_guard) = self.worker_handle.lock() {
            if let Some(handle) = handle_guard.take() {
                let _ = handle.join();
            }
        }
    }

    fn nudge_signal(&self, sig: WorkerSignal) {
        if let Ok(guard) = self.worker_tx.lock() {
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(sig);
            }
        }
    }

    fn emit_current_snapshot(&self) {
        let snap_opt = self.shared.lock().ok().map(|g| g.snapshot.clone());
        if let (Some(app), Some(snap)) = (&self.app_handle, snap_opt) {
            let _ = app.emit("bridge://status", snap);
        }
    }

    fn restart_worker(&self) {
        self.shutdown();

        let (profile_opt, backend_mode, backend_url, access_tier) = {
            let Ok(guard) = self.shared.lock() else {
                return;
            };
            (
                guard.config.fs25_profile_path.clone(),
                guard.config.backend_mode,
                guard.config.backend_url.clone(),
                guard.config.access_tier,
            )
        };

        let default_market_name = match backend_mode {
            BackendMode::Mock => "Local Simulation (Mock)".to_string(),
            BackendMode::Live => "Global Market".to_string(),
        };
        let default_mock_logo = match backend_mode {
            BackendMode::Mock => self.mock_backend.simulated_logo(),
            BackendMode::Live => None,
        };

        let Some(raw_profile) = profile_opt else {
            self.update_snapshot(|snap| {
                snap.state = BridgeConnectionState::PathMissing;
                snap.game_edition = None;
                snap.access_tier = access_tier;
                snap.backend_mode = backend_mode;
                snap.backend_url = backend_url;
                snap.market_name = default_market_name;
                snap.market_logo_url = None;
                snap.bridge_path = None;
                snap.heartbeat_seq = 0;
                snap.last_heartbeat_unix = None;
                snap.last_error =
                    Some("No Farming Simulator profile folder found.".to_string());
            });
            if let Some(ref app) = self.app_handle {
                crate::tray::sync_window_and_tray_edition(app, None);
            }
            self.push_i18n_log(
                LogLevel::Warn,
                "logPathMissing",
                [],
                "No Farming Simulator profile folder found – please select manually.".to_string(),
            );
            return;
        };

        let paths = match resolve_bridge_paths(Path::new(&raw_profile)) {
            Ok(p) => p,
            Err(err) => {
                self.update_snapshot(|snap| {
                    snap.state = BridgeConnectionState::PathMissing;
                    snap.game_edition = None;
                    snap.access_tier = access_tier;
                    snap.backend_mode = backend_mode;
                    snap.backend_url = backend_url;
                    snap.market_name = default_market_name;
                    snap.market_logo_url = None;
                    snap.bridge_path = None;
                    snap.heartbeat_seq = 0;
                    snap.last_heartbeat_unix = None;
                    snap.last_error = Some(err.clone());
                });
                if let Some(ref app) = self.app_handle {
                    crate::tray::sync_window_and_tray_edition(app, None);
                }
                push_raw_log_shared(&self.shared, &self.app_handle, LogLevel::Error, err);
                return;
            }
        };

        if let Err(err) = paths.ensure_directories() {
            self.update_snapshot(|snap| {
                snap.state = BridgeConnectionState::PathMissing;
                snap.heartbeat_seq = 0;
                snap.last_heartbeat_unix = None;
                snap.last_error = Some(err.clone());
            });
            push_raw_log_shared(&self.shared, &self.app_handle, LogLevel::Error, err);
            return;
        }

        // Clear any leftover `gmDataSync_*.xml` snapshots from previous sessions before starting fresh.
        prune_market_data_xmls(&paths.read_dir, 0);

        if let Ok(mut paths_guard) = self.active_paths.lock() {
            *paths_guard = Some(paths.clone());
        }

        let detected_ed = paths.game_edition.clone();
        self.update_snapshot(|snap| {
            snap.state = BridgeConnectionState::Connecting;
            snap.game_edition = Some(detected_ed.clone());
            snap.access_tier = access_tier;
            snap.backend_mode = backend_mode;
            snap.backend_url = backend_url;
            snap.market_name = default_market_name;
            snap.market_logo_url = default_mock_logo;
            snap.profile_path = Some(paths.profile_dir.to_string_lossy().into_owned());
            snap.bridge_path = Some(paths.bridge_root.to_string_lossy().into_owned());
            snap.heartbeat_seq = 0;
            snap.last_heartbeat_unix = None;
            snap.last_error = None;
        });
        if let Some(ref app) = self.app_handle {
            crate::tray::sync_window_and_tray_edition(app, Some(&detected_ed));
        }

        let backend: Arc<dyn BackendService> = match backend_mode {
            BackendMode::Mock => Arc::clone(&self.mock_backend) as Arc<dyn BackendService>,
            BackendMode::Live => Arc::clone(&self.live_backend) as Arc<dyn BackendService>,
        };

        let (tx, rx) = mpsc::channel::<WorkerSignal>();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_worker = Arc::clone(&stop_flag);
        let shared_clone = Arc::clone(&self.shared);
        let app_clone = self.app_handle.clone();

        let handle = thread::Builder::new()
            .name("gm-client-bridge".to_string())
            .spawn({
                let watcher_tx = tx.clone();
                move || {
                    run_bridge_worker(
                        paths,
                        backend,
                        rx,
                        watcher_tx,
                        stop_flag_worker,
                        shared_clone,
                        app_clone,
                    );
                }
            })
            .expect("failed to spawn bridge worker thread");

        if let Ok(mut flag_guard) = self.worker_stop_flag.lock() {
            *flag_guard = Some(stop_flag);
        }
        if let Ok(mut tx_guard) = self.worker_tx.lock() {
            *tx_guard = Some(tx);
        }
        if let Ok(mut h_guard) = self.worker_handle.lock() {
            *h_guard = Some(handle);
        }
    }

    fn update_snapshot<F: FnOnce(&mut BridgeSnapshot)>(&self, updater: F) {
        let payload = if let Ok(mut guard) = self.shared.lock() {
            updater(&mut guard.snapshot);
            Some(guard.snapshot.clone())
        } else {
            None
        };
        if let (Some(app), Some(snap)) = (&self.app_handle, payload) {
            let _ = app.emit("bridge://status", snap);
        }
    }

    fn push_i18n_log<const N: usize>(
        &self,
        level: LogLevel,
        key: &str,
        params: [(&str, String); N],
        fallback_en: String,
    ) {
        push_i18n_log_shared(&self.shared, &self.app_handle, level, key, params, fallback_en);
    }
}

impl Drop for BridgeManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn run_bridge_worker(
    paths: BridgePaths,
    backend: Arc<dyn BackendService>,
    rx: mpsc::Receiver<WorkerSignal>,
    watcher_tx: Sender<WorkerSignal>,
    stop_flag: Arc<AtomicBool>,
    shared: Arc<Mutex<SharedState>>,
    app_handle: Option<AppHandle>,
) {
    // Keep `_watcher` alive for the lifetime of this worker thread.
    // Even if `notify` misses an event on OneDrive/Proton, `FALLBACK_SCAN_INTERVAL` (500 ms) catches it.
    let mut _watcher: Option<RecommendedWatcher> =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(ev) = res {
                if ev.kind.is_create() || ev.kind.is_modify() {
                    let _ = watcher_tx.send(WorkerSignal::Wakeup);
                }
            }
        })
        .ok();

    if let Some(ref mut w) = _watcher {
        let _ = w.watch(&paths.write_dir, RecursiveMode::NonRecursive);
    }

    let mut poll_interval = Duration::from_secs(10);
    let mut min_version = "1.0.0".to_string();
    let mut is_connected = false;
    let mut version_ok = true;

    let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
    let mut last_market_poll = Instant::now() - Duration::from_secs(60);
    let mut last_status_check = Instant::now() - Duration::from_secs(60);

    let mut last_synced_items: Vec<MarketItemDto> = Vec::new();
    let mut known_titles: HashMap<String, String> = HashMap::new();
    let mut whitelist_lang: Option<String> = None;
    let mut known_great_demands: HashSet<String> = HashSet::new();
    let mut initialized_gd_baseline = false;

    let bridge_display = paths.bridge_root.display().to_string();
    push_i18n_log_shared(
        &shared,
        &app_handle,
        LogLevel::Info,
        "logWatchingBridge",
        [("path", bridge_display.clone())],
        format!("Watching bridge folder: {bridge_display}"),
    );

    loop {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }
        let mut force_sync = false;
        let mut refresh_l10n_only = false;
        match rx.recv_timeout(FALLBACK_SCAN_INTERVAL) {
            Ok(WorkerSignal::Stop) => break,
            Ok(WorkerSignal::ForceSyncNow) => force_sync = true,
            Ok(WorkerSignal::RefreshLocalization) => refresh_l10n_only = true,
            Ok(WorkerSignal::Wakeup) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }

        // Automatically expire temporary Support/Diagnostic (`Extended`) mode after 30 minutes.
        check_and_expire_extended_tier_shared(&shared, &app_handle);

        if refresh_l10n_only && !last_synced_items.is_empty() {
            let active_lang = active_ui_language(&shared);
            let (gd_titles, gd_codes) = collect_great_demand_labels(
                &last_synced_items,
                &known_titles,
                whitelist_lang.as_deref(),
                &active_lang,
            );
            let (r_cnt, w_cnt) = count_bridge_queues(&paths);
            emit_snapshot_update(&shared, &app_handle, |snap| {
                snap.active_great_demands = gd_titles;
                snap.active_great_demand_codes = gd_codes;
                snap.read_queue_count = r_cnt;
                snap.write_queue_count = w_cnt;
            });
        }

        // 1. Handshake / Backend reachability & minVersion check (every 10s, or immediately on ForceSyncNow / offline)
        let status_refresh_due =
            force_sync || !is_connected || last_status_check.elapsed() >= Duration::from_secs(10);

        if status_refresh_due {
            last_status_check = Instant::now();
            let status_res = backend.check_status();
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }
            match status_res {
                Ok(hs) => {
                    min_version = hs.min_exe_version.clone();
                    poll_interval = Duration::from_secs(hs.poll_interval_sec.clamp(2, 120));
                    version_ok = is_version_sufficient(PROTOCOL_EXE_VERSION, &min_version);
                    let next_connected = hs.online && version_ok;

                    let prev_state = shared.lock().ok().map(|g| g.snapshot.state);
                    let next_state = if !version_ok {
                        BridgeConnectionState::Outdated
                    } else if next_connected {
                        BridgeConnectionState::Online
                    } else {
                        BridgeConnectionState::BackendOffline
                    };

                    if prev_state != Some(next_state) {
                        match next_state {
                            BridgeConnectionState::Online => push_i18n_log_shared(
                                &shared,
                                &app_handle,
                                LogLevel::Success,
                                "logConnected",
                                [
                                    ("market", hs.market_name.clone()),
                                    ("minVersion", min_version.clone()),
                                ],
                                format!(
                                    "Connected to '{}' (minVersion {}).",
                                    hs.market_name, min_version
                                ),
                            ),
                            BridgeConnectionState::Outdated => push_i18n_log_shared(
                                &shared,
                                &app_handle,
                                LogLevel::Error,
                                "logOutdated",
                                [
                                    ("currentVersion", PROTOCOL_EXE_VERSION.to_string()),
                                    ("minVersion", min_version.clone()),
                                ],
                                format!(
                                    "App version outdated (v{PROTOCOL_EXE_VERSION} < v{min_version}). Please update!"
                                ),
                            ),
                            BridgeConnectionState::BackendOffline => push_i18n_log_shared(
                                &shared,
                                &app_handle,
                                LogLevel::Warn,
                                "logMarketOffline",
                                [],
                                "Market temporarily offline.".to_string(),
                            ),
                            _ => {}
                        }
                    }

                    let changed_conn = is_connected != next_connected;
                    is_connected = next_connected;
                    let (r_cnt, w_cnt) = count_bridge_queues(&paths);

                    emit_snapshot_update(&shared, &app_handle, |snap| {
                        snap.state = next_state;
                        snap.market_name = hs.market_name;
                        snap.market_logo_url = hs.market_logo_url;
                        snap.api_version = Some(hs.api_version);
                        snap.min_exe_version = min_version.clone();
                        snap.read_queue_count = r_cnt;
                        snap.write_queue_count = w_cnt;
                        snap.last_error = if !version_ok {
                            Some(format!("Update required (min v{min_version})"))
                        } else {
                            None
                        };
                    });

                    if changed_conn {
                        last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
                    }
                }
                Err(err) => {
                    let was_online = is_connected;
                    is_connected = false;
                    let err_msg = err.to_string();

                    let should_log = shared
                        .lock()
                        .ok()
                        .map(|g| {
                            g.snapshot.state != BridgeConnectionState::BackendOffline
                                || g.snapshot.last_error.as_deref() != Some(err_msg.as_str())
                        })
                        .unwrap_or(true);

                    if should_log {
                        emit_classified_backend_error(
                            &err,
                            LogLevel::Warn,
                            &shared,
                            &app_handle,
                        );
                    }

                    let (r_cnt, w_cnt) = count_bridge_queues(&paths);
                    emit_snapshot_update(&shared, &app_handle, |snap| {
                        snap.state = BridgeConnectionState::BackendOffline;
                        snap.read_queue_count = r_cnt;
                        snap.write_queue_count = w_cnt;
                        snap.last_error = Some(err_msg);
                    });

                    if was_online {
                        last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
                    }
                }
            }
        }

        // 2. Heartbeat file (`globalMarketOnline.xml`) every 3.5s.
        // Even when Outdated or BackendOffline, write `isConnected="false"` + `minVersion`
        // so Lua immediately enforces `minVersion` or displays the red HUD icon.
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }
        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            last_heartbeat = Instant::now();
            let hb_connected = is_connected && version_ok;
            match write_heartbeat_xml(
                &paths.staging_dir,
                &paths.online_xml_file,
                hb_connected,
                PROTOCOL_EXE_VERSION,
                &min_version,
            ) {
                Ok(()) if hb_connected => {
                    let now_secs = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let next_seq = if let Ok(mut guard) = shared.lock() {
                        guard.snapshot.heartbeat_seq = guard.snapshot.heartbeat_seq.wrapping_add(1);
                        guard.snapshot.last_heartbeat_unix = Some(now_secs);
                        guard.snapshot.heartbeat_seq
                    } else {
                        1
                    };
                    if let Some(ref app) = app_handle {
                        let _ = app.emit(
                            "bridge://heartbeat",
                            HeartbeatTickPayload {
                                seq: next_seq,
                                timestamp_unix: now_secs,
                            },
                        );
                    }
                }
                Ok(()) => {}
                Err(e) => {
                    push_raw_log_shared(&shared, &app_handle, LogLevel::Error, e);
                }
            }
        }

        // Only process incoming `write/*.xml` and market state syncs when connected & version-valid.
        if !is_connected || !version_ok {
            continue;
        }

        // 3. Process outgoing Lua files in `write/` (whitelists first, then transactions in chronological order).
        let pending_files = list_sorted_write_xmls(&paths.write_dir);
        for xml_path in pending_files {
            let Some(raw_xml) = read_string_with_retry(&xml_path) else {
                continue;
            };
            let tx_id = extract_tx_id(&xml_path);

            match parse_incoming_write_xml(&raw_xml, &tx_id) {
                Ok(IncomingPacket::UpdateWhitelist(wl_req)) => {
                    whitelist_lang = Some(wl_req.lang.clone());
                    for wl_item in &wl_req.items {
                        let key = wl_item.name.trim().to_ascii_uppercase();
                        let title = wl_item.title.trim();
                        if !key.is_empty() && !title.is_empty() {
                            known_titles.insert(key, title.to_string());
                        }
                    }
                    let total = wl_req.items.len();
                    let lang = wl_req.lang.clone();
                    match backend.sync_whitelist(wl_req) {
                        Ok(state) => {
                            let _ = fs::remove_file(&xml_path);
                            apply_and_write_market_state(
                                &paths,
                                &state.items,
                                true,
                                &known_titles,
                                whitelist_lang.as_deref(),
                                &mut last_synced_items,
                                &mut known_great_demands,
                                &mut initialized_gd_baseline,
                                &shared,
                                &app_handle,
                            );
                            let (approved, pending, rejected) = count_statuses(&state.items);
                            push_i18n_log_shared(
                                &shared,
                                &app_handle,
                                LogLevel::Success,
                                "logWhitelistSynced",
                                [
                                    ("lang", lang.clone()),
                                    ("total", total.to_string()),
                                    ("approved", approved.to_string()),
                                    ("pending", pending.to_string()),
                                    ("rejected", rejected.to_string()),
                                ],
                                format!(
                                    "Whitelist ({lang}, {total} map items) synced -> {approved} approved, {pending} pending, {rejected} offline-only."
                                ),
                            );
                            last_market_poll = Instant::now();
                        }
                        Err(err) => {
                            handle_backend_io_error(&err, &mut is_connected, &shared, &app_handle);
                            break;
                        }
                    }
                }
                Ok(IncomingPacket::Transactions(tx_list)) => {
                    let mut all_ok = true;
                    let mut latest_state: Option<Vec<MarketItemDto>> = None;
                    let (client_id, ui_lang) = shared
                        .lock()
                        .map(|g| (g.config.client_id.clone(), g.config.language.clone()))
                        .unwrap_or_else(|_| (String::new(), "en".to_string()));

                    for mut tx in tx_list {
                        tx.tx_id = qualify_transaction_id(&client_id, &tx.tx_id);
                        let display_item = resolve_fill_type_code(
                            &tx.fill_type,
                            None,
                            &known_titles,
                            whitelist_lang.as_deref(),
                            &ui_lang,
                        );
                        let tx_type_str = tx.tx_type.as_str().to_string();
                        let amount_str = tx.amount.to_string();
                        let code_str = tx.fill_type.clone();

                        match backend.submit_transaction(tx) {
                            Ok(state) => {
                                push_i18n_log_shared(
                                    &shared,
                                    &app_handle,
                                    LogLevel::Success,
                                    "logTransactionBooked",
                                    [
                                        ("txType", tx_type_str.clone()),
                                        ("amount", amount_str.clone()),
                                        ("item", display_item.clone()),
                                        ("itemCode", code_str.clone()),
                                    ],
                                    format!(
                                        "Transaction booked: {tx_type_str} {amount_str} L {code_str}"
                                    ),
                                );
                                latest_state = Some(state.items);
                            }
                            Err(err) => {
                                all_ok = false;
                                handle_backend_io_error(
                                    &err,
                                    &mut is_connected,
                                    &shared,
                                    &app_handle,
                                );
                                break;
                            }
                        }
                    }

                    if all_ok {
                        let _ = fs::remove_file(&xml_path);
                        if let Some(items) = latest_state {
                            apply_and_write_market_state(
                                &paths,
                                &items,
                                true,
                                &known_titles,
                                whitelist_lang.as_deref(),
                                &mut last_synced_items,
                                &mut known_great_demands,
                                &mut initialized_gd_baseline,
                                &shared,
                                &app_handle,
                            );
                            last_market_poll = Instant::now();
                        }
                    } else {
                        break;
                    }
                }
                Ok(IncomingPacket::IgnoredUnknownAction(act)) => {
                    let _ = fs::remove_file(&xml_path);
                    push_i18n_log_shared(
                        &shared,
                        &app_handle,
                        LogLevel::Info,
                        "logUnknownActionSkipped",
                        [("action", act.clone())],
                        format!("Skipped unknown XML action '{act}'."),
                    );
                }
                Err(parse_err) => {
                    // Remove corrupted XML so it cannot block the queue forever.
                    let _ = fs::remove_file(&xml_path);
                    let file_label = xml_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown.xml")
                        .to_string();
                    push_i18n_log_shared(
                        &shared,
                        &app_handle,
                        LogLevel::Error,
                        "logCorruptXmlDiscarded",
                        [("file", file_label.clone()), ("error", parse_err.clone())],
                        format!("Discarded malformed XML file '{file_label}': {parse_err}"),
                    );
                }
            }
        }

        // 4. Periodic market state poll.
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }
        if is_connected && (force_sync || last_market_poll.elapsed() >= poll_interval) {
            last_market_poll = Instant::now();
            let market_res = backend.fetch_market_state();
            if stop_flag.load(Ordering::SeqCst) {
                break;
            }
            match market_res {
                Ok(state) => {
                    apply_and_write_market_state(
                        &paths,
                        &state.items,
                        force_sync,
                        &known_titles,
                        whitelist_lang.as_deref(),
                        &mut last_synced_items,
                        &mut known_great_demands,
                        &mut initialized_gd_baseline,
                        &shared,
                        &app_handle,
                    );
                }
                Err(err) => {
                    handle_backend_io_error(&err, &mut is_connected, &shared, &app_handle);
                }
            }
        }
    }

    // Always clean up `globalMarketOnline.xml` and unread `read/gmDataSync_*.xml` snapshots when leaving the worker loop.
    cleanup_bridge_runtime_files(&paths.online_xml_file, &paths.read_dir, &paths.staging_dir);
}

#[allow(clippy::too_many_arguments)]
fn apply_and_write_market_state(
    paths: &BridgePaths,
    items: &[MarketItemDto],
    force_write_xml: bool,
    known_titles: &HashMap<String, String>,
    whitelist_lang: Option<&str>,
    last_synced_items: &mut Vec<MarketItemDto>,
    known_great_demands: &mut HashSet<String>,
    initialized_gd_baseline: &mut bool,
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) {
    let (notify_enabled, active_lang) = shared
        .lock()
        .map(|g| (g.config.notifications_enabled, g.config.language.clone()))
        .unwrap_or_else(|_| (true, "en".to_string()));

    // Detect newly started Great Demands for optional desktop notifications.
    let current_gds: HashSet<String> = items
        .iter()
        .filter(|i| i.status == FillTypeStatus::Approved && i.great_demand)
        .map(|i| i.fill_type.clone())
        .collect();

    if *initialized_gd_baseline {
        for item in items {
            if item.status == FillTypeStatus::Approved
                && item.great_demand
                && !known_great_demands.contains(&item.fill_type)
            {
                let localized_title = resolve_fill_type_code(
                    &item.fill_type,
                    item.title.as_deref(),
                    known_titles,
                    whitelist_lang,
                    &active_lang,
                );
                let bonus_percent = (item.great_demand_bonus * 100.0).round().max(0.0) as u32;

                push_i18n_log_shared(
                    shared,
                    app_handle,
                    LogLevel::Info,
                    "logGreatDemandStarted",
                    [
                        ("item", localized_title.clone()),
                        ("itemCode", item.fill_type.clone()),
                        ("bonus", bonus_percent.to_string()),
                    ],
                    format!(
                        "Great Demand started: {} (+{bonus_percent}%)!",
                        item.fill_type
                    ),
                );

                if notify_enabled {
                    if let Some(app) = app_handle {
                        crate::tray::send_great_demand_os_notification(
                            app,
                            &active_lang,
                            Some(&paths.game_edition),
                            &localized_title,
                            bonus_percent,
                        );
                        let _ = app.emit(
                            "bridge://great-demand",
                            GreatDemandNotification {
                                fill_type: item.fill_type.clone(),
                                title: localized_title,
                                bonus_percent,
                            },
                        );
                    }
                }
            }
        }
    }
    *known_great_demands = current_gds;
    *initialized_gd_baseline = true;

    let data_changed = force_write_xml || last_synced_items.as_slice() != items;
    if data_changed {
        match write_market_data_xml(&paths.staging_dir, &paths.read_dir, items) {
            Ok(_) => {
                *last_synced_items = items.to_vec();
            }
            Err(err) => {
                push_raw_log_shared(shared, app_handle, LogLevel::Error, err);
                return;
            }
        }
    }

    let (approved, pending, rejected) = count_statuses(items);
    let (gd_titles, gd_codes) =
        collect_great_demand_labels(items, known_titles, whitelist_lang, &active_lang);
    let (r_cnt, w_cnt) = count_bridge_queues(paths);

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    emit_snapshot_update(shared, app_handle, |snap| {
        snap.approved_count = approved;
        snap.pending_count = pending;
        snap.rejected_count = rejected;
        snap.read_queue_count = r_cnt;
        snap.write_queue_count = w_cnt;
        snap.active_great_demands = gd_titles;
        snap.active_great_demand_codes = gd_codes;
        snap.last_sync_unix = Some(now_secs);
    });
}

fn active_ui_language(shared: &Arc<Mutex<SharedState>>) -> String {
    shared
        .lock()
        .map(|g| g.config.language.clone())
        .unwrap_or_else(|_| "en".to_string())
}

fn collect_great_demand_labels(
    items: &[MarketItemDto],
    known_titles: &HashMap<String, String>,
    whitelist_lang: Option<&str>,
    active_lang: &str,
) -> (Vec<String>, Vec<String>) {
    let mut pairs: Vec<(String, String)> = items
        .iter()
        .filter(|i| i.status == FillTypeStatus::Approved && i.great_demand)
        .map(|i| {
            let label = resolve_fill_type_code(
                &i.fill_type,
                i.title.as_deref(),
                known_titles,
                whitelist_lang,
                active_lang,
            );
            (label, i.fill_type.clone())
        })
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let titles = pairs.iter().map(|(l, _)| l.clone()).collect();
    let codes = pairs.into_iter().map(|(_, c)| c).collect();
    (titles, codes)
}

/// Resolves the best display title for `fill_type` in `active_lang`:
/// 1. Built-in 5-language FS25 basegame dictionary (`localize_fill_type`) whenever `whitelist_lang`
///    differs from `active_lang` (so switching UI language always translates known crops!).
/// 2. Mod-provided `whitelist` title when playing in the same language (or for custom Mod-Map crops!).
/// 3. Raw uppercase `fill_type` code as final fallback.
fn resolve_fill_type_code(
    fill_type: &str,
    dto_title: Option<&str>,
    known_titles: &HashMap<String, String>,
    whitelist_lang: Option<&str>,
    active_lang: &str,
) -> String {
    let wl_matches_ui = whitelist_lang
        .map(|wl| wl.eq_ignore_ascii_case(active_lang))
        .unwrap_or(false);

    if wl_matches_ui {
        if let Some(t) = known_titles.get(fill_type).filter(|s| !s.trim().is_empty()) {
            return t.clone();
        }
        if let Some(t) = dto_title.filter(|s| !s.trim().is_empty()) {
            return t.to_string();
        }
    }

    if let Some(localized) = localize_fill_type(fill_type, active_lang) {
        return localized.to_string();
    }

    if let Some(t) = known_titles.get(fill_type).filter(|s| !s.trim().is_empty()) {
        return t.clone();
    }
    if let Some(t) = dto_title.filter(|s| !s.trim().is_empty()) {
        return t.to_string();
    }

    fill_type.to_string()
}

fn count_bridge_queues(paths: &BridgePaths) -> (usize, usize) {
    (
        count_xmls_in_dir(&paths.read_dir),
        count_xmls_in_dir(&paths.write_dir),
    )
}

fn count_xmls_in_dir(dir: &Path) -> usize {
    fs::read_dir(dir)
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
                })
                .count()
        })
        .unwrap_or(0)
}

fn count_statuses(items: &[MarketItemDto]) -> (usize, usize, usize) {
    let mut approved = 0;
    let mut pending = 0;
    let mut rejected = 0;
    for item in items {
        match item.status {
            FillTypeStatus::Approved => approved += 1,
            FillTypeStatus::Pending => pending += 1,
            FillTypeStatus::Rejected => rejected += 1,
        }
    }
    (approved, pending, rejected)
}

fn emit_classified_backend_error(
    err: &ApiError,
    default_network_level: LogLevel,
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) {
    match err {
        ApiError::SlotLimitReached { max_slots } => {
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Error,
                "logTokenSlotLimitReached",
                [("maxSlots", max_slots.to_string())],
                format!(
                    "Key slot limit reached: All {max_slots} active player slots for this Community/Logo key are currently in use."
                ),
            );
        }
        ApiError::Unauthorized => {
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Error,
                "logTokenInvalid",
                [],
                "Community/Logo key is invalid or revoked (HTTP 401/403) – please check Settings."
                    .to_string(),
            );
        }
        _ => {
            let msg = err.to_string();
            push_i18n_log_shared(
                shared,
                app_handle,
                default_network_level,
                "logBackendUnreachable",
                [("error", msg.clone())],
                format!("Backend unreachable ({msg}) – automatic reconnect active."),
            );
        }
    }
}

fn handle_backend_io_error(
    err: &ApiError,
    is_connected: &mut bool,
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) {
    *is_connected = false;
    let msg = err.to_string();
    emit_classified_backend_error(err, LogLevel::Error, shared, app_handle);
    emit_snapshot_update(shared, app_handle, |snap| {
        snap.state = BridgeConnectionState::BackendOffline;
        snap.last_error = Some(msg);
    });
}

fn list_sorted_write_xmls(write_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(write_dir) else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("xml"))
        })
        .collect();

    // Prioritize `whitelist_*.xml` before `transaction_*.xml`, then sort lexicographically by filename.
    files.sort_by(|a, b| {
        let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let a_is_wl = a_name.starts_with("whitelist_");
        let b_is_wl = b_name.starts_with("whitelist_");
        b_is_wl.cmp(&a_is_wl).then_with(|| a_name.cmp(b_name))
    });

    files
}

fn read_string_with_retry(path: &Path) -> Option<String> {
    for attempt in 0..4 {
        match fs::read_to_string(path) {
            Ok(content) if !content.trim().is_empty() => return Some(content),
            _ => {
                if attempt < 3 {
                    thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }
    None
}

fn extract_tx_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    stem.strip_prefix("transaction_")
        .unwrap_or(stem)
        .to_string()
}

fn emit_snapshot_update<F: FnOnce(&mut BridgeSnapshot)>(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
    updater: F,
) {
    let snap_opt = if let Ok(mut guard) = shared.lock() {
        updater(&mut guard.snapshot);
        Some(guard.snapshot.clone())
    } else {
        None
    };
    if let (Some(app), Some(snap)) = (app_handle, snap_opt) {
        let _ = app.emit("bridge://status", snap);
    }
}

fn push_i18n_log_shared<const N: usize>(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
    level: LogLevel,
    key: &str,
    params: [(&str, String); N],
    fallback_en: String,
) {
    let map: BTreeMap<String, String> = params
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

    append_log_entry(
        shared,
        app_handle,
        LogEntry {
            timestamp_ms: now_epoch_ms(),
            level,
            message: fallback_en,
            key: Some(key.to_string()),
            params: if map.is_empty() { None } else { Some(map) },
        },
    );
}

fn push_raw_log_shared(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
    level: LogLevel,
    message: String,
) {
    append_log_entry(
        shared,
        app_handle,
        LogEntry {
            timestamp_ms: now_epoch_ms(),
            level,
            message,
            key: None,
            params: None,
        },
    );
}

fn append_log_entry(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
    entry: LogEntry,
) {
    if let Ok(mut guard) = shared.lock() {
        if guard.logs.len() >= MAX_LOG_ENTRIES {
            guard.logs.pop_front();
        }
        guard.logs.push_back(entry.clone());
    }

    if let Some(app) = app_handle {
        let _ = app.emit("bridge://log", entry);
    }
}

fn check_and_expire_extended_tier_shared(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) {
    let expired_snap = if let Ok(mut guard) = shared.lock() {
        let is_due = guard.config.access_tier == AccessTier::Extended
            && guard
                .extended_expires_at
                .is_some_and(|deadline| Instant::now() >= deadline);
        if is_due {
            guard.extended_expires_at = None;
            guard.config.access_tier = AccessTier::User;
            guard.snapshot.access_tier = AccessTier::User;
            Some(guard.snapshot.clone())
        } else {
            None
        }
    } else {
        None
    };

    if let Some(snap) = expired_snap {
        if let Some(app) = app_handle {
            let _ = app.emit("bridge://status", snap);
        }
        push_i18n_log_shared(
            shared,
            app_handle,
            LogLevel::Info,
            "logExtendedExpired",
            [],
            "Diagnostic mode automatically ended after 30 minutes.".to_string(),
        );
    }
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Standard RFC 4648 Base64 encoder used for local Mock community logo previews (`data:image/...;base64,...`).
pub fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn load_local_logo_as_data_url(file_path: &Path) -> Result<String, String> {
    const MAX_LOGO_BYTES: u64 = 8 * 1024 * 1024;

    if !file_path.is_file() {
        return Err(format!("Logo file not found: {}", file_path.display()));
    }

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "dds" {
        let mut cmd = std::process::Command::new("magick");
        cmd.arg(format!("{}[0]", file_path.display())).args([
            "-trim",
            "+repage",
            "-bordercolor",
            "none",
            "-border",
            "2",
            "-resize",
            "512x512>",
            "png:-",
        ]);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        let output = cmd.output().map_err(|_| {
            "Cannot convert .dds directly (ImageMagick 'magick' not found on PATH). Please convert the logo to .png or .webp first.".to_string()
        })?;

        if !output.status.success() || !output.stdout.starts_with(b"\x89PNG\r\n\x1a\n") {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to convert .dds logo: {}", stderr.trim()));
        }

        return Ok(format!(
            "data:image/png;base64,{}",
            encode_base64(&output.stdout)
        ));
    }

    let meta = fs::metadata(file_path).map_err(|e| format!("Cannot inspect logo file: {e}"))?;
    if meta.len() > MAX_LOGO_BYTES {
        return Err("Logo file exceeds 8 MB limit.".to_string());
    }

    let bytes = fs::read(file_path).map_err(|e| format!("Cannot read logo file: {e}"))?;
    let mime = match ext.as_str() {
        "png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
        "webp" if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" => {
            "image/webp"
        }
        "jpg" | "jpeg" if bytes.starts_with(b"\xFF\xD8\xFF") => "image/jpeg",
        "png" | "webp" | "jpg" | "jpeg" => {
            return Err("File header does not match its image extension.".to_string());
        }
        _ => {
            return Err(
                "Unsupported logo format. Please select a PNG, WebP, JPG, or DDS file.".to_string(),
            );
        }
    };

    Ok(format!("data:{mime};base64,{}", encode_base64(&bytes)))
}
