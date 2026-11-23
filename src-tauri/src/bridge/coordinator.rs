//! Central background bridge coordinator:
//! Combines `notify` event watching + `500 ms` OneDrive-safe fallback polling,
//! `3.5 s` `globalMarketOnline.xml` heartbeats, periodic backend state sync,
//! Great Demand transition detection, structured multi-language log events,
//! and instant shutdown cleanup.

use crate::api::fastapi::FastApiBackend;
use crate::api::mock::LocalMockBackend;
use crate::api::types::{
    ContractActionRequest, ContractActionResultDto, ContractStateResponse, ContractSubAction,
    FillTypeStatus, MarketItemDto, ServerHandshake, ServerNotice,
};
use crate::api::{ApiError, BackendService};
use crate::bridge::dds::{
    convert_logo_bytes_to_dds_file, decode_logo_rgba8, encode_rgba8_to_png_bytes,
    ensure_default_dsa_logo_dds, is_valid_cached_logo_dds, logo_cache_hash12,
    logo_rel_path_for_hash, MAX_LOGO_RAW_BYTES,
};
use crate::bridge::l10n::localize_fill_type;
use crate::bridge::notice::{
    classify_outage, message_languages, notice_for_maintenance_error, OfflineReason,
    StoredServerNotice,
};
use crate::bridge::paths::{discover_fs25_profile_path, resolve_bridge_paths, BridgePaths};
use crate::bridge::xml::{
    cleanup_bridge_runtime_files, cleanup_bridge_transfer_files, has_unread_contract_sync_xml,
    is_version_sufficient, parse_incoming_write_xml, prune_contract_sync_xmls,
    prune_market_data_xmls, write_contract_data_xml, write_heartbeat_xml_with_extras,
    write_market_data_xml_with_branding, HeartbeatBranding, HeartbeatExtras,
    HeartbeatMarketProfile, HeartbeatMarketProfiles, HeartbeatNotice, IncomingPacket,
};
use crate::config::{
    qualify_transaction_id, verify_access_key, AccessTier, AppConfig, BackendMode,
    SavedTokenProfile, TokenVault, DEFAULT_OFFICIAL_BACKEND_URL,
};
use crate::PROTOCOL_EXE_VERSION;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const MAX_LOG_ENTRIES: usize = 250;
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(3500);
const FALLBACK_SCAN_INTERVAL: Duration = Duration::from_millis(500);
/// Support/Diagnostic (`AccessTier::Extended`) mode automatically resets to `User` after 30 minutes.
pub const EXTENDED_TIER_TIMEOUT: Duration = Duration::from_secs(30 * 60);

const STOP_RUNNING: u8 = 0;
const STOP_SHUTDOWN: u8 = 1;
const STOP_RESTART_PRESERVE_HEARTBEAT: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BridgeConnectionState {
    PathMissing,
    Disconnected,
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
    pub connection_enabled: bool,
    /// Dynamically detected Farming Simulator edition (e.g. `"FS25"`, `"FS28"`), or `None` before selection.
    pub game_edition: Option<String>,
    pub access_tier: AccessTier,
    pub backend_mode: BackendMode,
    pub backend_url: String,
    pub api_version: Option<String>,
    pub market_name: String,
    pub branding_tier: u8,
    pub is_private_market: bool,
    pub market_logo_url: Option<String>,
    pub market_logo_file: Option<String>,
    pub active_token_id: Option<String>,
    pub saved_tokens: Vec<SavedTokenProfile>,
    pub profile_path: Option<String>,
    pub bridge_path: Option<String>,
    pub exe_version: String,
    pub min_exe_version: String,
    pub latest_exe_version: Option<String>,
    pub contracts_supported: bool,
    pub open_contracts_count: usize,
    pub open_offers_count: usize,
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
    /// Unix time (seconds) when the temporary Extended mode ends, `None` outside Extended mode.
    pub extended_expires_unix: Option<u64>,
    /// Current server notice (maintenance, restart or info; Spec Section 3.1.3).
    pub server_notice: Option<ServerNotice>,
    /// `maintenance` or `unreachable` while the backend is offline, otherwise `None`.
    pub offline_reason: Option<String>,
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
    token_vault: TokenVault,
    connection_enabled: bool,
    last_seen_server_id: Option<String>,
    snapshot: BridgeSnapshot,
    logs: VecDeque<LogEntry>,
    extended_expires_at: Option<Instant>,
    /// Last accepted in-game market switch request (`write/marketSwitch_*.xml`), for the 30 s rate limit.
    last_ingame_market_switch: Option<Instant>,
    /// Last server notice, persisted in `server_notice.json` (Spec Section 3.1.3).
    server_notice: StoredServerNotice,
    notice_file: PathBuf,
}

pub struct BridgeManager {
    app_handle: Option<AppHandle>,
    shared: Arc<Mutex<SharedState>>,
    mock_backend: Arc<LocalMockBackend>,
    live_backend: Arc<FastApiBackend>,
    worker_tx: Mutex<Option<Sender<WorkerSignal>>>,
    worker_stop_mode: Mutex<Option<Arc<AtomicU8>>>,
    worker_handle: Mutex<Option<JoinHandle<()>>>,
    active_paths: Mutex<Option<BridgePaths>>,
}

impl BridgeManager {
    pub fn new(config_file: PathBuf, app_handle: Option<AppHandle>) -> Self {
        let config_existed = config_file.is_file();
        let mut config = AppConfig::load_from_file(&config_file);
        let mut should_persist = !config_existed;

        let vault_path = TokenVault::vault_path_for_config(&config_file);
        let mut token_vault = TokenVault::load_or_empty(vault_path);

        // Crash-safe one-time migration: seal any legacy plaintext `marketToken` from older
        // `settings.json` files into `tokens.vault` first, then scrub `settings.json`.
        if let Some(legacy_raw) = config.market_token.take() {
            if !legacy_raw.trim().is_empty() {
                if let Ok(profile) = token_vault.upsert_token(&legacy_raw, None) {
                    if token_vault.save().is_ok() {
                        config.active_token_id = Some(profile.id);
                        should_persist = true;
                    }
                }
            } else {
                should_persist = true;
            }
        }

        if let Some(ref active_id) = config.active_token_id {
            if token_vault.get_profile(active_id).is_none() {
                config.active_token_id = None;
                should_persist = true;
            }
        }

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

        let initial_connection_enabled = config.auto_connect_on_launch;
        let initial_raw_token = config
            .active_token_id
            .as_deref()
            .and_then(|id| token_vault.resolve_raw_token(id));

        let live_backend = Arc::new(
            FastApiBackend::new(
                config.backend_url.clone(),
                initial_raw_token,
                config.client_id.clone(),
            )
            .expect("HTTP client initialization failed"),
        );
        let mock_backend = Arc::new(LocalMockBackend::new());

        let notice_file = StoredServerNotice::path_for_config(&config_file);
        let server_notice = StoredServerNotice::load(&notice_file, unix_now());

        let initial_snapshot = BridgeSnapshot {
            state: if initial_connection_enabled {
                BridgeConnectionState::PathMissing
            } else {
                BridgeConnectionState::Disconnected
            },
            connection_enabled: initial_connection_enabled,
            game_edition: None,
            access_tier: config.access_tier,
            backend_mode: config.backend_mode,
            backend_url: config.backend_url.clone(),
            api_version: None,
            market_name: match config.backend_mode {
                BackendMode::Mock => "Local Simulation (Mock)".to_string(),
                BackendMode::Live => "Global Market".to_string(),
            },
            branding_tier: 0,
            is_private_market: false,
            market_logo_url: None,
            market_logo_file: None,
            active_token_id: config.active_token_id.clone(),
            saved_tokens: token_vault.list_profiles(),
            profile_path: config.fs25_profile_path.clone(),
            bridge_path: None,
            exe_version: PROTOCOL_EXE_VERSION.to_string(),
            min_exe_version: "0.9.9".to_string(),
            latest_exe_version: None,
            contracts_supported: false,
            open_contracts_count: 0,
            open_offers_count: 0,
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
            extended_expires_unix: None,
            server_notice: server_notice.notice.clone(),
            offline_reason: None,
        };

        let mgr = Self {
            app_handle,
            shared: Arc::new(Mutex::new(SharedState {
                config,
                config_file,
                token_vault,
                connection_enabled: initial_connection_enabled,
                last_seen_server_id: None,
                snapshot: initial_snapshot,
                logs: VecDeque::with_capacity(MAX_LOG_ENTRIES),
                extended_expires_at: None,
                last_ingame_market_switch: None,
                server_notice,
                notice_file,
            })),
            mock_backend,
            live_backend,
            worker_tx: Mutex::new(None),
            worker_stop_mode: Mutex::new(None),
            worker_handle: Mutex::new(None),
            active_paths: Mutex::new(None),
        };

        mgr.restart_worker();
        if !initial_connection_enabled {
            mgr.push_i18n_log(
                LogLevel::Info,
                "logStartedDisconnected",
                [],
                "Started in disconnected mode – click 'Connect' in the top right to start market synchronization.".to_string(),
            );
        }
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

    pub fn is_connection_enabled(&self) -> bool {
        self.shared
            .lock()
            .map(|g| g.connection_enabled)
            .unwrap_or(true)
    }

    /// Manually connects (`enabled == true`) or disconnects (`enabled == false`) the bridge worker.
    /// When disconnecting, stops HTTP polling immediately and removes `globalMarketOnline.xml` +
    /// unread `read/gmDataSync_*.xml` files so FS25 cleanly transitions to local Offline mode.
    pub fn set_connection_enabled(&self, enabled: bool) -> BridgeSnapshot {
        let changed = {
            let Ok(mut guard) = self.shared.lock() else {
                return self.get_snapshot();
            };
            if guard.connection_enabled == enabled {
                return guard.snapshot.clone();
            }
            guard.connection_enabled = enabled;
            guard.snapshot.connection_enabled = enabled;
            true
        };

        if changed {
            if enabled {
                self.push_i18n_log(
                    LogLevel::Info,
                    "logConnectingByUser",
                    [],
                    "Manual connect requested – starting market synchronization...".to_string(),
                );
            } else {
                self.push_i18n_log(
                    LogLevel::Info,
                    "logDisconnectedByUser",
                    [],
                    "Disconnected manually – market synchronization paused.".to_string(),
                );
            }
            self.restart_worker();
            if let Some(ref app) = self.app_handle {
                let cfg = self.get_config();
                let snap = self.get_snapshot();
                crate::tray::update_tray_state(
                    app,
                    &cfg.language,
                    snap.game_edition.as_deref(),
                    snap.connection_enabled,
                );
            }
        }
        self.get_snapshot()
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
            let next_path_str = persisted_dir.to_string_lossy().into_owned();
            if guard.config.fs25_profile_path.as_deref() != Some(next_path_str.as_str()) {
                guard.last_seen_server_id = None;
            }
            guard.config.fs25_profile_path = Some(next_path_str);
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

    /// Updates user configuration (language, theme, notifications, tray behavior, backend mode/URL, or active token profile).
    pub fn update_config(&self, mut next: AppConfig) -> Result<BridgeSnapshot, String> {
        next.language = crate::config::normalize_language(&next.language);
        next.theme = crate::config::normalize_theme(&next.theme);

        let (needs_worker_restart, lang_changed, logo_changed) = {
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

            // If a caller supplied a legacy/direct `market_token` string, vault it immediately
            // and never retain plaintext on `AppConfig`.
            if let Some(raw_tok) = next.market_token.take() {
                let trimmed = raw_tok.trim();
                if !trimmed.is_empty() {
                    let profile = guard.token_vault.upsert_token(trimmed, None)?;
                    guard.token_vault.save()?;
                    next.active_token_id = Some(profile.id);
                }
            }

            if let Some(ref req_id) = next.active_token_id {
                let cleaned = req_id.trim().to_string();
                if cleaned.is_empty() {
                    next.active_token_id = None;
                } else if guard.token_vault.get_profile(&cleaned).is_some() {
                    next.active_token_id = Some(cleaned);
                } else {
                    next.active_token_id = guard.config.active_token_id.clone();
                }
            }

            let resolved_raw_token = next
                .active_token_id
                .as_deref()
                .and_then(|id| guard.token_vault.resolve_raw_token(id));

            let lang_diff = guard.config.language != next.language;
            let logo_diff = guard.config.show_official_logo != next.show_official_logo;
            if guard.config.fs25_profile_path != next.fs25_profile_path {
                guard.last_seen_server_id = None;
            }
            let restart = guard.config.backend_mode != next.backend_mode
                || guard.config.backend_url != next.backend_url
                || guard.config.active_token_id != next.active_token_id
                || guard.config.fs25_profile_path != next.fs25_profile_path;

            self.live_backend.update_connection_config(
                next.backend_url.clone(),
                resolved_raw_token,
                next.client_id.clone(),
            );

            guard.snapshot.access_tier = next.access_tier;
            guard.snapshot.backend_mode = next.backend_mode;
            guard.snapshot.backend_url = next.backend_url.clone();
            guard.snapshot.active_token_id = next.active_token_id.clone();
            guard.snapshot.saved_tokens = guard.token_vault.list_profiles();

            guard.config = next;
            guard.config.save_to_file(&guard.config_file)?;
            (restart, lang_diff, logo_diff)
        };

        if needs_worker_restart {
            self.restart_worker();
        } else {
            if lang_changed {
                self.nudge_signal(WorkerSignal::RefreshLocalization);
            }
            if logo_changed {
                // Re-evaluate the heartbeat branding right away instead of on the next 10 s status check
                self.nudge_signal(WorkerSignal::ForceSyncNow);
            }
            if !lang_changed && !logo_changed {
                self.emit_current_snapshot();
            }
        }
        Ok(self.get_snapshot())
    }

    /// Seals a new (or updated) market/logo token inside the OS-protected `TokenVault`,
    /// selects it as the active profile, and immediately switches the live connection.
    pub fn add_market_token(
        &self,
        raw_token: &str,
        custom_label: Option<&str>,
    ) -> Result<BridgeSnapshot, String> {
        let (masked_hint, should_restart) = {
            let mut guard = self
                .shared
                .lock()
                .map_err(|_| "Internal state lock poisoned".to_string())?;

            let profile = guard.token_vault.upsert_token(raw_token, custom_label)?;
            guard.token_vault.save()?;

            let raw_resolved = guard.token_vault.resolve_raw_token(&profile.id);
            guard.config.active_token_id = Some(profile.id.clone());
            guard.config.save_to_file(&guard.config_file)?;

            self.live_backend.update_connection_config(
                guard.config.backend_url.clone(),
                raw_resolved,
                guard.config.client_id.clone(),
            );

            guard.snapshot.active_token_id = Some(profile.id.clone());
            guard.snapshot.saved_tokens = guard.token_vault.list_profiles();

            (
                profile.masked_hint,
                guard.config.backend_mode == BackendMode::Live,
            )
        };

        self.push_i18n_log(
            LogLevel::Success,
            "logTokenSavedAndActivated",
            [("hint", masked_hint.clone())],
            format!("Saved and activated market token ({masked_hint})."),
        );

        if should_restart {
            self.restart_worker();
        } else {
            self.emit_current_snapshot();
        }
        Ok(self.get_snapshot())
    }

    /// Switches between the Official Global Market (`token_id == None`) and any saved token profile.
    pub fn select_active_market_token(
        &self,
        token_id: Option<String>,
    ) -> Result<BridgeSnapshot, String> {
        let cleaned_id = token_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let (changed, selected_profile, should_restart) = {
            let mut guard = self
                .shared
                .lock()
                .map_err(|_| "Internal state lock poisoned".to_string())?;

            let (resolved_tok, prof_opt) = if let Some(ref id) = cleaned_id {
                let Some(prof) = guard.token_vault.get_profile(id) else {
                    return Err("Unknown market token profile.".to_string());
                };
                let Some(raw) = guard.token_vault.resolve_raw_token(id) else {
                    return Err("Unknown market token profile.".to_string());
                };
                (Some(raw), Some(prof))
            } else {
                (None, None)
            };

            if guard.config.active_token_id == cleaned_id {
                return Ok(guard.snapshot.clone());
            }

            guard.config.active_token_id = cleaned_id.clone();
            guard.config.save_to_file(&guard.config_file)?;

            self.live_backend.update_connection_config(
                guard.config.backend_url.clone(),
                resolved_tok,
                guard.config.client_id.clone(),
            );

            guard.snapshot.active_token_id = cleaned_id;
            guard.snapshot.saved_tokens = guard.token_vault.list_profiles();

            (
                true,
                prof_opt,
                guard.config.backend_mode == BackendMode::Live,
            )
        };

        if changed {
            if let Some(prof) = selected_profile {
                let display = prof
                    .custom_label
                    .or(prof.detected_market_name)
                    .unwrap_or_else(|| prof.masked_hint.clone());
                self.push_i18n_log(
                    LogLevel::Info,
                    "logMarketProfileSelected",
                    [("profile", display.clone())],
                    format!("Switched active market token to '{display}'."),
                );
            } else {
                self.push_i18n_log(
                    LogLevel::Info,
                    "logMarketSwitchedOfficial",
                    [],
                    "Switched to Official Global Market (no token).".to_string(),
                );
            }

            if should_restart {
                self.restart_worker();
            } else {
                self.emit_current_snapshot();
            }
        }
        Ok(self.get_snapshot())
    }

    /// Updates the optional user-defined nickname on an existing saved token profile.
    pub fn update_market_token_label(
        &self,
        token_id: &str,
        custom_label: Option<&str>,
    ) -> Result<BridgeSnapshot, String> {
        {
            let mut guard = self
                .shared
                .lock()
                .map_err(|_| "Internal state lock poisoned".to_string())?;
            if guard.token_vault.update_custom_label(token_id, custom_label) {
                guard.token_vault.save()?;
                guard.snapshot.saved_tokens = guard.token_vault.list_profiles();
            }
        }
        self.emit_current_snapshot();
        Ok(self.get_snapshot())
    }

    /// Deletes a saved token profile from the OS vault. If it was currently active, cleanly
    /// switches back to the Official Global Market (`Tier 0`).
    pub fn remove_market_token(&self, token_id: &str) -> Result<BridgeSnapshot, String> {
        let clean_target = token_id.trim();
        let (removed, was_active, masked_hint, should_restart) = {
            let mut guard = self
                .shared
                .lock()
                .map_err(|_| "Internal state lock poisoned".to_string())?;

            let hint = guard
                .token_vault
                .get_profile(clean_target)
                .map(|p| p.masked_hint)
                .unwrap_or_else(|| "••••••".to_string());
            let was_removed = guard.token_vault.remove_token(clean_target);
            if !was_removed {
                return Ok(guard.snapshot.clone());
            }
            guard.token_vault.save()?;

            let active_hit = guard.config.active_token_id.as_deref() == Some(clean_target);
            if active_hit {
                guard.config.active_token_id = None;
                guard.config.save_to_file(&guard.config_file)?;
                self.live_backend.update_connection_config(
                    guard.config.backend_url.clone(),
                    None,
                    guard.config.client_id.clone(),
                );
                guard.snapshot.active_token_id = None;
            }
            guard.snapshot.saved_tokens = guard.token_vault.list_profiles();

            (
                true,
                active_hit,
                hint,
                active_hit && guard.config.backend_mode == BackendMode::Live,
            )
        };

        if removed {
            self.push_i18n_log(
                LogLevel::Info,
                "logTokenRemoved",
                [("hint", masked_hint.clone())],
                format!("Removed saved market token ({masked_hint})."),
            );
            if should_restart {
                self.restart_worker();
            } else if was_active {
                self.emit_current_snapshot();
            } else {
                self.emit_current_snapshot();
            }
        }
        Ok(self.get_snapshot())
    }

    /// Generates an anonymized Markdown diagnostic report with all vault tokens scrubbed.
    pub fn generate_diagnostic_report(&self) -> String {
        check_and_expire_extended_tier_shared(&self.shared, &self.app_handle);
        let (snap, cfg, logs, secrets) = match self.shared.lock() {
            Ok(g) => (
                g.snapshot.clone(),
                g.config.clone(),
                g.logs.iter().cloned().collect::<Vec<_>>(),
                g.token_vault.all_raw_tokens(),
            ),
            Err(e) => {
                let g = e.into_inner();
                (
                    g.snapshot.clone(),
                    g.config.clone(),
                    g.logs.iter().cloned().collect::<Vec<_>>(),
                    g.token_vault.all_raw_tokens(),
                )
            }
        };
        crate::diagnostics::build_diagnostic_report_with_secrets(&snap, &cfg, &logs, &secrets)
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
            guard.snapshot.extended_expires_unix = match tier {
                AccessTier::Extended => Some(now_epoch_ms() / 1000 + extended_ttl.as_secs()),
                AccessTier::User | AccessTier::Developer => None,
            };

            if tier != AccessTier::Developer {
                if guard.config.backend_mode != BackendMode::Live
                    || guard.config.backend_url != DEFAULT_OFFICIAL_BACKEND_URL
                {
                    guard.config.backend_mode = BackendMode::Live;
                    guard.config.backend_url = DEFAULT_OFFICIAL_BACKEND_URL.to_string();
                    let active_tok = guard
                        .config
                        .active_token_id
                        .as_deref()
                        .and_then(|id| guard.token_vault.resolve_raw_token(id));
                    self.live_backend.update_connection_config(
                        guard.config.backend_url.clone(),
                        active_tok,
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
            guard.snapshot.extended_expires_unix = None;
            guard.config.access_tier = AccessTier::User;
            guard.config.backend_mode = BackendMode::Live;
            guard.config.backend_url = DEFAULT_OFFICIAL_BACKEND_URL.to_string();
            let active_tok = guard
                .config
                .active_token_id
                .as_deref()
                .and_then(|id| guard.token_vault.resolve_raw_token(id));
            self.live_backend.update_connection_config(
                guard.config.backend_url.clone(),
                active_tok,
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

    pub fn mock_set_latest_version(&self, latest_version: Option<String>) {
        if self.get_config().access_tier != AccessTier::Developer {
            return;
        }
        let cleaned = latest_version
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        self.mock_backend
            .set_simulated_latest_version(cleaned.clone());
        match cleaned {
            Some(ver) => {
                self.push_i18n_log(
                    LogLevel::Info,
                    "logMockLatestVersion",
                    [("latestVersion", ver.clone())],
                    format!("Simulation: Optional update version set to v{ver}."),
                );
            }
            None => {
                self.push_i18n_log(
                    LogLevel::Info,
                    "logMockLatestVersionReset",
                    [],
                    "Simulation: Optional update announcement cleared.".to_string(),
                );
            }
        }
        self.nudge_signal(WorkerSignal::ForceSyncNow);
    }

    /// Steps the simulated server maintenance in Developer Mock mode (Spec Section 3.1.3):
    /// `0` = no notice, `1` = maintenance announced (in 15 min), `2` = maintenance running (`503`).
    pub fn mock_set_server_notice_stage(&self, stage: u8) {
        if self.get_config().access_tier != AccessTier::Developer {
            return;
        }
        let stage = stage.min(2);
        self.mock_backend.set_simulated_notice_stage(stage);
        let (key, fallback) = match stage {
            0 => ("logMockNoticeCleared", "Simulation: Server notice cleared."),
            1 => ("logMockNoticeAnnounced", "Simulation: Maintenance announced."),
            _ => ("logMockMaintenanceStarted", "Simulation: Maintenance started."),
        };
        self.push_i18n_log(LogLevel::Info, key, [], fallback.to_string());
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
                self.mock_backend.set_simulated_logo(None, None);
                let next_tier = self.mock_backend.simulated_branding_tier();
                if cfg.backend_mode == BackendMode::Mock {
                    self.update_snapshot(|snap| {
                        snap.branding_tier = next_tier;
                        snap.is_private_market = matches!(next_tier, 2 | 3);
                        snap.market_logo_url = None;
                        snap.market_logo_file = None;
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
                let (data_url, raw_bytes) = load_local_logo_with_bytes(file_path)?;
                let file_label = file_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("logo")
                    .to_string();

                self.mock_backend
                    .set_simulated_logo(Some(data_url.clone()), Some(raw_bytes));
                let next_tier = self.mock_backend.simulated_branding_tier();
                if cfg.backend_mode == BackendMode::Mock {
                    self.update_snapshot(|snap| {
                        snap.branding_tier = next_tier;
                        snap.is_private_market = matches!(next_tier, 2 | 3);
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

    /// Sets the simulated 3-tier market branding (`0..=3`) and optional private market name in Developer Mock mode.
    pub fn mock_set_branding_tier(
        &self,
        tier: u8,
        market_name: Option<String>,
    ) -> Result<BridgeSnapshot, String> {
        let cfg = self.get_config();
        if cfg.access_tier != AccessTier::Developer {
            return Err("Developer mode required.".to_string());
        }

        let clamped_tier = tier.min(3);
        self.mock_backend
            .set_simulated_branding_tier(clamped_tier, market_name);

        if cfg.backend_mode == BackendMode::Mock {
            let sim_logo = self.mock_backend.simulated_logo();
            self.update_snapshot(|snap| {
                snap.branding_tier = clamped_tier;
                snap.is_private_market = matches!(clamped_tier, 2 | 3);
                snap.market_logo_url = sim_logo;
                if matches!(clamped_tier, 0 | 2) {
                    snap.market_logo_file = None;
                }
            });
        }

        self.push_i18n_log(
            LogLevel::Info,
            "logMockBrandingTier",
            [("brandingTier", clamped_tier.to_string())],
            format!("Simulation: Branding tier set to Tier {clamped_tier}."),
        );

        self.nudge_signal(WorkerSignal::ForceSyncNow);
        Ok(self.get_snapshot())
    }

    /// Triggers foreign contract fulfillment / direct-offer purchase & spawns fresh listings in Developer Mock mode.
    pub fn mock_simulate_contract_activity(&self) -> Option<String> {
        if self.get_config().access_tier != AccessTier::Developer {
            return None;
        }
        let summary = self.mock_backend.simulate_contract_activity();
        self.push_i18n_log(
            LogLevel::Success,
            "logMockContractSim",
            [("summary", summary.clone())],
            format!(
                "Simulation: Triggered online contract & direct offer event ({summary})."
            ),
        );
        self.nudge_signal(WorkerSignal::ForceSyncNow);
        Some(summary)
    }

    /// Signals the worker to stop immediately, cleans up `globalMarketOnline.xml` + `read/gmDataSync_*.xml`
    /// synchronously, and returns the worker `JoinHandle` so GUI shutdown never blocks the main event loop.
    pub fn prepare_fast_exit(&self) -> Option<JoinHandle<()>> {
        if let Ok(mut mode_guard) = self.worker_stop_mode.lock() {
            if let Some(mode) = mode_guard.take() {
                mode.store(STOP_SHUTDOWN, Ordering::SeqCst);
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
        if let Ok(mut mode_guard) = self.worker_stop_mode.lock() {
            if let Some(mode) = mode_guard.take() {
                mode.store(STOP_SHUTDOWN, Ordering::SeqCst);
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

    /// Stops the active worker for an internal live restart (e.g. token/profile or backend switch).
    /// When restarting on the same `bridge_root`, `globalMarketOnline.xml` is intentionally preserved
    /// so Dedicated Server Lua polling never observes a transient missing heartbeat file while the new
    /// worker performs its HTTPS handshake; only unread `read/gmDataSync_*.xml` and `.staging/*.tmp`
    /// files are cleared. If switching to a different profile folder (or `None`), the previous folder
    /// receives a full runtime cleanup including `globalMarketOnline.xml`.
    fn stop_worker_for_restart(&self, preserve_for_bridge_root: Option<&Path>) {
        let prev_paths = self
            .active_paths
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());

        let preserve_heartbeat = prev_paths.as_ref().is_some_and(|prev| {
            preserve_for_bridge_root.is_some_and(|next_root| next_root == prev.bridge_root.as_path())
        });
        let target_stop_mode = if preserve_heartbeat {
            STOP_RESTART_PRESERVE_HEARTBEAT
        } else {
            STOP_SHUTDOWN
        };

        if let Ok(mut mode_guard) = self.worker_stop_mode.lock() {
            if let Some(mode) = mode_guard.take() {
                mode.store(target_stop_mode, Ordering::SeqCst);
            }
        }
        if let Ok(mut tx_guard) = self.worker_tx.lock() {
            if let Some(tx) = tx_guard.take() {
                let _ = tx.send(WorkerSignal::Stop);
            }
        }
        if let Ok(mut handle_guard) = self.worker_handle.lock() {
            if let Some(handle) = handle_guard.take() {
                let _ = handle.join();
            }
        }
        if let Some(paths) = prev_paths {
            if preserve_heartbeat {
                cleanup_bridge_transfer_files(&paths.read_dir, &paths.staging_dir);
            } else {
                cleanup_bridge_runtime_files(
                    &paths.online_xml_file,
                    &paths.read_dir,
                    &paths.staging_dir,
                );
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
        let (
            profile_opt,
            backend_mode,
            backend_url,
            access_tier,
            active_saved_profile,
            connection_enabled,
        ) = {
            let Ok(guard) = self.shared.lock() else {
                return;
            };
            let active_prof = guard
                .config
                .active_token_id
                .as_deref()
                .and_then(|id| guard.token_vault.get_profile(id));
            (
                guard.config.fs25_profile_path.clone(),
                guard.config.backend_mode,
                guard.config.backend_url.clone(),
                guard.config.access_tier,
                active_prof,
                guard.connection_enabled,
            )
        };

        let resolved_paths = profile_opt
            .as_deref()
            .map(|raw_profile| resolve_bridge_paths(Path::new(raw_profile)));
        // Preserve `globalMarketOnline.xml` only while connected and staying on the exact same bridge folder;
        // disconnecting (`!connection_enabled`) always performs full cleanup so FS25 transitions cleanly to offline.
        let preserve_root = if connection_enabled {
            resolved_paths
                .as_ref()
                .and_then(|res| res.as_ref().ok())
                .map(|p| p.bridge_root.as_path())
        } else {
            None
        };

        self.stop_worker_for_restart(preserve_root);

        let default_market_name = match backend_mode {
            BackendMode::Mock => "Local Simulation (Mock)".to_string(),
            BackendMode::Live => active_saved_profile
                .as_ref()
                .filter(|p| p.is_private_market)
                .and_then(|p| p.detected_market_name.clone())
                .unwrap_or_else(|| "Global Market".to_string()),
        };
        let default_mock_tier = match backend_mode {
            BackendMode::Mock => self.mock_backend.simulated_branding_tier(),
            BackendMode::Live => active_saved_profile
                .as_ref()
                .and_then(|p| p.branding_tier)
                .unwrap_or(0),
        };
        let default_mock_logo = match backend_mode {
            BackendMode::Mock => self.mock_backend.simulated_logo(),
            BackendMode::Live => active_saved_profile
                .as_ref()
                .filter(|p| matches!(p.branding_tier, Some(1 | 3)))
                .and_then(|p| p.cached_logo_url.clone()),
        };

        let Some(paths_res) = resolved_paths else {
            self.update_snapshot(|snap| {
                snap.state = BridgeConnectionState::PathMissing;
                snap.connection_enabled = connection_enabled;
                snap.game_edition = None;
                snap.access_tier = access_tier;
                snap.backend_mode = backend_mode;
                snap.backend_url = backend_url;
                snap.api_version = None;
                snap.market_name = default_market_name;
                snap.branding_tier = 0;
                snap.is_private_market = false;
                snap.market_logo_url = None;
                snap.market_logo_file = None;
                snap.bridge_path = None;
                snap.contracts_supported = false;
                snap.open_contracts_count = 0;
                snap.open_offers_count = 0;
                snap.approved_count = 0;
                snap.pending_count = 0;
                snap.rejected_count = 0;
                snap.active_great_demands.clear();
                snap.active_great_demand_codes.clear();
                snap.heartbeat_seq = 0;
                snap.last_heartbeat_unix = None;
                snap.last_sync_unix = None;
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

        let paths = match paths_res {
            Ok(p) => p,
            Err(err) => {
                self.update_snapshot(|snap| {
                    snap.state = BridgeConnectionState::PathMissing;
                    snap.connection_enabled = connection_enabled;
                    snap.game_edition = None;
                    snap.access_tier = access_tier;
                    snap.backend_mode = backend_mode;
                    snap.backend_url = backend_url;
                    snap.api_version = None;
                    snap.market_name = default_market_name;
                    snap.branding_tier = 0;
                    snap.is_private_market = false;
                    snap.market_logo_url = None;
                    snap.market_logo_file = None;
                    snap.bridge_path = None;
                    snap.contracts_supported = false;
                    snap.open_contracts_count = 0;
                    snap.open_offers_count = 0;
                    snap.approved_count = 0;
                    snap.pending_count = 0;
                    snap.rejected_count = 0;
                    snap.active_great_demands.clear();
                    snap.active_great_demand_codes.clear();
                    snap.heartbeat_seq = 0;
                    snap.last_heartbeat_unix = None;
                    snap.last_sync_unix = None;
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
            cleanup_bridge_runtime_files(&paths.online_xml_file, &paths.read_dir, &paths.staging_dir);
            self.update_snapshot(|snap| {
                snap.state = BridgeConnectionState::PathMissing;
                snap.connection_enabled = connection_enabled;
                snap.api_version = None;
                snap.contracts_supported = false;
                snap.open_contracts_count = 0;
                snap.open_offers_count = 0;
                snap.approved_count = 0;
                snap.pending_count = 0;
                snap.rejected_count = 0;
                snap.active_great_demands.clear();
                snap.active_great_demand_codes.clear();
                snap.heartbeat_seq = 0;
                snap.last_heartbeat_unix = None;
                snap.last_sync_unix = None;
                snap.last_error = Some(err.clone());
            });
            push_raw_log_shared(&self.shared, &self.app_handle, LogLevel::Error, err);
            return;
        }

        let detected_ed = paths.game_edition.clone();
        let remembered_logo_dds = if backend_mode == BackendMode::Live {
            active_saved_profile
                .as_ref()
                .and_then(|p| p.cached_logo_file.as_deref())
                .filter(|rel| is_valid_cached_logo_dds(&paths.bridge_root.join(rel)))
                .map(ToOwned::to_owned)
        } else {
            None
        };

        if !connection_enabled {
            // Ensure any crash-leftover heartbeat or snapshot files are cleaned up immediately.
            cleanup_bridge_runtime_files(&paths.online_xml_file, &paths.read_dir, &paths.staging_dir);
            let (r_cnt, w_cnt) = count_bridge_queues(&paths);
            self.update_snapshot(|snap| {
                snap.state = BridgeConnectionState::Disconnected;
                snap.connection_enabled = false;
                snap.game_edition = Some(detected_ed.clone());
                snap.access_tier = access_tier;
                snap.backend_mode = backend_mode;
                snap.backend_url = backend_url;
                snap.api_version = None;
                snap.market_name = default_market_name;
                snap.branding_tier = default_mock_tier;
                snap.is_private_market = matches!(default_mock_tier, 2 | 3);
                snap.market_logo_url = default_mock_logo;
                snap.market_logo_file = remembered_logo_dds;
                snap.profile_path = Some(paths.profile_dir.to_string_lossy().into_owned());
                snap.bridge_path = Some(paths.bridge_root.to_string_lossy().into_owned());
                snap.contracts_supported = false;
                snap.open_contracts_count = 0;
                snap.open_offers_count = 0;
                snap.approved_count = 0;
                snap.pending_count = 0;
                snap.rejected_count = 0;
                snap.read_queue_count = r_cnt;
                snap.write_queue_count = w_cnt;
                snap.active_great_demands.clear();
                snap.active_great_demand_codes.clear();
                snap.heartbeat_seq = 0;
                snap.last_heartbeat_unix = None;
                snap.last_sync_unix = None;
                snap.last_error = None;
            });
            if let Some(ref app) = self.app_handle {
                crate::tray::sync_window_and_tray_edition(app, Some(&detected_ed));
            }
            return;
        }

        // Clear any leftover `gmDataSync_*.xml` & `gmContractSync_*.xml` snapshots from previous sessions before starting fresh.
        prune_market_data_xmls(&paths.read_dir, 0);
        prune_contract_sync_xmls(&paths.read_dir, 0);

        if let Ok(mut paths_guard) = self.active_paths.lock() {
            *paths_guard = Some(paths.clone());
        }

        self.update_snapshot(|snap| {
            snap.state = BridgeConnectionState::Connecting;
            snap.connection_enabled = true;
            snap.game_edition = Some(detected_ed.clone());
            snap.access_tier = access_tier;
            snap.backend_mode = backend_mode;
            snap.backend_url = backend_url;
            snap.api_version = None;
            snap.market_name = default_market_name;
            snap.branding_tier = default_mock_tier;
            snap.is_private_market = matches!(default_mock_tier, 2 | 3);
            snap.market_logo_url = default_mock_logo;
            snap.market_logo_file = remembered_logo_dds;
            snap.profile_path = Some(paths.profile_dir.to_string_lossy().into_owned());
            snap.bridge_path = Some(paths.bridge_root.to_string_lossy().into_owned());
            snap.contracts_supported = false;
            snap.open_contracts_count = 0;
            snap.open_offers_count = 0;
            snap.approved_count = 0;
            snap.pending_count = 0;
            snap.rejected_count = 0;
            snap.active_great_demands.clear();
            snap.active_great_demand_codes.clear();
            snap.heartbeat_seq = 0;
            snap.last_heartbeat_unix = None;
            snap.last_sync_unix = None;
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
        // Kick off the initial handshake immediately instead of waiting for the first 500 ms poll timeout.
        let _ = tx.send(WorkerSignal::Wakeup);

        let stop_mode = Arc::new(AtomicU8::new(STOP_RUNNING));
        let stop_mode_worker = Arc::clone(&stop_mode);
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
                        stop_mode_worker,
                        shared_clone,
                        app_clone,
                    );
                }
            })
            .expect("failed to spawn bridge worker thread");

        if let Ok(mut mode_guard) = self.worker_stop_mode.lock() {
            *mode_guard = Some(stop_mode);
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
    stop_mode: Arc<AtomicU8>,
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
    let mut min_version = "0.9.9".to_string();
    let mut latest_version: Option<String> = None;
    let mut announced_optional_version: Option<String> = None;
    let mut is_connected = false;
    let mut version_ok = true;
    let mut contracts_supported = false;
    let mut server_notice: Option<ServerNotice> = shared
        .lock()
        .ok()
        .and_then(|g| g.server_notice.notice.clone());
    let mut offline_reason: Option<OfflineReason> = None;

    let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
    let mut last_market_poll = Instant::now() - Duration::from_secs(60);
    let mut last_contract_poll = Instant::now() - Duration::from_secs(60);
    let mut last_status_check = Instant::now() - Duration::from_secs(60);
    let mut retry_hold_until: Option<Instant> = None;

    let mut last_synced_items: Vec<MarketItemDto> = Vec::new();
    let mut known_titles: HashMap<String, String> = HashMap::new();
    let mut whitelist_lang: Option<String> = None;
    let mut known_great_demands: HashSet<String> = HashSet::new();
    let mut initialized_gd_baseline = false;
    let mut active_branding = HeartbeatBranding::default();
    let mut failed_logo_attempts: HashMap<String, Instant> = HashMap::new();
    let mut pending_branding_sync = false;

    let mut active_server_id: Option<String> = shared
        .lock()
        .ok()
        .and_then(|g| g.last_seen_server_id.clone());
    let mut has_synced_contracts_once = false;
    let mut last_synced_contract_state = ContractStateResponse::default();
    let mut last_contract_write_at = Instant::now() - Duration::from_secs(60);
    let mut delivered_action_ids: VecDeque<String> = VecDeque::new();
    let mut announced_settlement_ids: HashSet<String> = HashSet::new();

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
        if stop_mode.load(Ordering::SeqCst) != STOP_RUNNING {
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
        if stop_mode.load(Ordering::SeqCst) != STOP_RUNNING {
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

        if force_sync {
            retry_hold_until = None;
        }

        // 1. Handshake / Backend reachability & minVersion check (every 10s when online, 5s when offline, or paused during CLIENT_IP_LOCKED)
        let hold_active = retry_hold_until.is_some_and(|until| Instant::now() < until);
        let status_refresh_due = if hold_active {
            false
        } else {
            retry_hold_until = None;
            force_sync
                || (!is_connected && last_status_check.elapsed() >= Duration::from_secs(5))
                || last_status_check.elapsed() >= Duration::from_secs(10)
        };

        if status_refresh_due {
            last_status_check = Instant::now();
            let status_res = backend.check_status();
            if stop_mode.load(Ordering::SeqCst) != STOP_RUNNING {
                break;
            }
            match status_res {
                Ok(hs) => {
                    retry_hold_until = None;
                    let next_min_version = hs.min_exe_version.clone();
                    let next_latest_version = hs
                        .latest_exe_version
                        .clone()
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty());
                    let changed_versions = min_version != next_min_version
                        || latest_version != next_latest_version;

                    min_version = next_min_version;
                    latest_version = next_latest_version;
                    poll_interval = Duration::from_secs(hs.poll_interval_sec.clamp(2, 120));
                    version_ok = is_version_sufficient(PROTOCOL_EXE_VERSION, &min_version);
                    let next_connected = hs.online && version_ok;

                    let now = unix_now();
                    let changed_notice = apply_server_notice(
                        &shared,
                        &app_handle,
                        &mut server_notice,
                        hs.notice.clone().filter(|n| !n.is_expired(now)),
                    );
                    // A backend that answers but reports `online: false` inside an announced window is in maintenance.
                    let next_offline_reason = (!next_connected
                        && version_ok
                        && server_notice
                            .as_ref()
                            .is_some_and(|n| n.covers_outage_at(now)))
                    .then_some(OfflineReason::Maintenance);
                    let changed_reason = offline_reason != next_offline_reason;
                    offline_reason = next_offline_reason;

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

                    if next_connected {
                        if let Some(ref lat) = latest_version {
                            if !is_version_sufficient(PROTOCOL_EXE_VERSION, lat) {
                                if announced_optional_version.as_deref() != Some(lat.as_str()) {
                                    announced_optional_version = Some(lat.clone());
                                    push_i18n_log_shared(
                                        &shared,
                                        &app_handle,
                                        LogLevel::Info,
                                        "logOptionalUpdateAvailable",
                                        [
                                            ("latestVersion", lat.clone()),
                                            ("currentVersion", PROTOCOL_EXE_VERSION.to_string()),
                                        ],
                                        format!(
                                            "Optional app update available: v{lat} (installed: v{PROTOCOL_EXE_VERSION}) – online trading continues normally."
                                        ),
                                    );
                                }
                            } else {
                                announced_optional_version = None;
                            }
                        } else {
                            announced_optional_version = None;
                        }
                    }

                    let custom_logo_rel = if next_connected {
                        resolve_or_cache_handshake_logo(
                            &paths,
                            backend.as_ref(),
                            &hs,
                            &mut failed_logo_attempts,
                            &shared,
                            &app_handle,
                        )
                    } else {
                        None
                    };

                    // Whenever no custom Community/Logo token is active (Tier 0 Official Market or Tier 2 Private Market,
                    // or if a custom logo is temporarily unavailable), automatically supply the built-in DSA logo `.dds`
                    // so FS25 renders the official DSA logo in-game without any server stream or mod patch.
                    // Players can switch the official logo off (`show_official_logo`); custom logos stay untouched.
                    let show_official_logo = shared
                        .lock()
                        .map(|g| g.config.show_official_logo)
                        .unwrap_or(true);
                    let effective_logo_rel = if next_connected {
                        custom_logo_rel.clone().or_else(|| {
                            show_official_logo
                                .then(|| {
                                    ensure_default_dsa_logo_dds(
                                        &paths.staging_dir,
                                        &paths.logos_dir,
                                    )
                                })
                                .flatten()
                        })
                    } else {
                        None
                    };

                    let ui_branding_tier = if next_connected {
                        match hs.branding_tier {
                            1 if custom_logo_rel.is_some() => 1,
                            1 => 0,
                            2 => 2,
                            3 if custom_logo_rel.is_some() => 3,
                            3 => 2,
                            _ => 0,
                        }
                    } else {
                        0
                    };

                    let next_branding = if next_connected {
                        let xml_tier = match (hs.is_private_market, effective_logo_rel.is_some()) {
                            (false, true) => 1,
                            (false, false) => 0,
                            (true, true) => 3,
                            (true, false) => 2,
                        };
                        HeartbeatBranding {
                            branding_tier: xml_tier,
                            is_private_market: hs.is_private_market,
                            market_name: hs.is_private_market.then(|| hs.market_name.clone()),
                            has_custom_logo: effective_logo_rel.is_some(),
                            logo_file: effective_logo_rel,
                        }
                        .normalized()
                    } else {
                        HeartbeatBranding::default()
                    };

                    let next_contracts_supported = next_connected && hs.contracts_supported;
                    let changed_conn = is_connected != next_connected;
                    let changed_branding = active_branding != next_branding;
                    let changed_contracts_sup = contracts_supported != next_contracts_supported;
                    is_connected = next_connected;
                    contracts_supported = next_contracts_supported;
                    active_branding = next_branding.clone();
                    let (r_cnt, w_cnt) = count_bridge_queues(&paths);

                    let ui_logo_url = matches!(ui_branding_tier, 1 | 3)
                        .then(|| hs.market_logo_url)
                        .flatten();

                    if next_connected {
                        if let Ok(mut guard) = shared.lock() {
                            if guard.config.backend_mode == BackendMode::Live {
                                if let Some(active_id) = guard.config.active_token_id.clone() {
                                    if guard.token_vault.record_handshake_metadata(
                                        &active_id,
                                        ui_branding_tier,
                                        hs.is_private_market,
                                        hs.is_private_market.then_some(hs.market_name.as_str()),
                                        ui_logo_url.as_deref(),
                                        custom_logo_rel.as_deref(),
                                    ) {
                                        let _ = guard.token_vault.save();
                                        guard.snapshot.saved_tokens =
                                            guard.token_vault.list_profiles();
                                    }
                                }
                            }
                        }
                    }

                    emit_snapshot_update(&shared, &app_handle, |snap| {
                        snap.state = next_state;
                        snap.market_name = hs.market_name;
                        snap.branding_tier = ui_branding_tier;
                        snap.is_private_market = hs.is_private_market;
                        snap.market_logo_url = ui_logo_url;
                        snap.market_logo_file = next_branding.logo_file.clone();
                        snap.api_version = Some(hs.api_version);
                        snap.min_exe_version = min_version.clone();
                        snap.latest_exe_version = latest_version.clone();
                        snap.contracts_supported = contracts_supported;
                        if !contracts_supported {
                            snap.open_contracts_count = 0;
                            snap.open_offers_count = 0;
                        }
                        snap.read_queue_count = r_cnt;
                        snap.write_queue_count = w_cnt;
                        snap.last_error = if !version_ok {
                            Some(format!("Update required (min v{min_version})"))
                        } else {
                            None
                        };
                        snap.offline_reason = offline_reason.map(|r| r.as_str().to_string());
                    });

                    if changed_branding && next_connected {
                        pending_branding_sync = true;
                    }
                    if changed_conn
                        || changed_branding
                        || changed_versions
                        || changed_contracts_sup
                        || changed_notice
                        || changed_reason
                    {
                        last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
                    }
                }
                Err(err) => {
                    if let ApiError::ClientIpLocked { retry_after_sec } = &err {
                        retry_hold_until = Some(
                            Instant::now()
                                + Duration::from_secs((*retry_after_sec).clamp(1, 86_400)),
                        );
                    }
                    let was_online = is_connected;
                    is_connected = false;
                    contracts_supported = false;
                    active_branding = HeartbeatBranding::default();
                    let err_msg = err.to_string();

                    let now = unix_now();
                    let next_notice = match &err {
                        ApiError::Maintenance { notice_id, until } => {
                            Some(notice_for_maintenance_error(
                                server_notice.as_ref(),
                                notice_id.as_deref(),
                                *until,
                                now,
                            ))
                        }
                        _ => server_notice.clone().filter(|n| !n.is_expired(now)),
                    };
                    let changed_notice =
                        apply_server_notice(&shared, &app_handle, &mut server_notice, next_notice);
                    let next_offline_reason = classify_outage(&err, server_notice.as_ref(), now);
                    let entering_maintenance = next_offline_reason
                        == Some(OfflineReason::Maintenance)
                        && offline_reason != Some(OfflineReason::Maintenance);
                    let changed_reason = offline_reason != next_offline_reason;
                    offline_reason = next_offline_reason;
                    if entering_maintenance {
                        log_maintenance_started(&shared, &app_handle, server_notice.as_ref());
                    }

                    // During maintenance every retry fails; the single maintenance entry above is enough.
                    let should_log = offline_reason != Some(OfflineReason::Maintenance)
                        && shared
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
                        snap.branding_tier = 0;
                        snap.is_private_market = false;
                        snap.market_logo_url = None;
                        snap.market_logo_file = None;
                        snap.contracts_supported = false;
                        snap.open_contracts_count = 0;
                        snap.open_offers_count = 0;
                        snap.read_queue_count = r_cnt;
                        snap.write_queue_count = w_cnt;
                        snap.last_error = Some(err_msg);
                        snap.offline_reason = offline_reason.map(|r| r.as_str().to_string());
                    });

                    if was_online || changed_notice || changed_reason {
                        last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
                    }
                }
            }
        }

        // 2. Heartbeat file (`globalMarketOnline.xml`) every 3.5s.
        // Even when Outdated or BackendOffline, write `isConnected="false"` + `minVersion`
        // so Lua immediately enforces `minVersion` or displays the red HUD icon.
        if stop_mode.load(Ordering::SeqCst) != STOP_RUNNING {
            break;
        }
        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            last_heartbeat = Instant::now();
            let hb_connected = is_connected && version_ok;
            let market_profiles = if hb_connected {
                heartbeat_market_profiles(&shared)
            } else {
                None
            };
            let hb_notice = server_notice.as_ref().map(|n| {
                let langs =
                    message_languages(whitelist_lang.as_deref(), &active_ui_language(&shared));
                heartbeat_notice(n, &langs)
            });
            match write_heartbeat_xml_with_extras(
                &paths.staging_dir,
                &paths.online_xml_file,
                hb_connected,
                PROTOCOL_EXE_VERSION,
                &min_version,
                latest_version.as_deref(),
                hb_connected.then_some(&active_branding),
                hb_connected && contracts_supported,
                HeartbeatExtras {
                    market_profiles: market_profiles.as_ref(),
                    server_notice: hb_notice.as_ref(),
                    offline_reason: offline_reason
                        .filter(|_| !hb_connected)
                        .map(OfflineReason::as_str),
                },
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
                            apply_and_write_market_state(
                                &paths,
                                &state.items,
                                true,
                                &known_titles,
                                whitelist_lang.as_deref(),
                                &mut last_synced_items,
                                &mut known_great_demands,
                                &mut initialized_gd_baseline,
                                Some(&active_branding),
                                &shared,
                                &app_handle,
                            );
                            pending_branding_sync = false;
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
                            let _ = fs::remove_file(&xml_path);
                            last_market_poll = Instant::now();
                        }
                        Err(err) => {
                            handle_backend_io_error(
                                &err,
                                &mut is_connected,
                                &mut retry_hold_until,
                                &shared,
                                &app_handle,
                            );
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
                                    &mut retry_hold_until,
                                    &shared,
                                    &app_handle,
                                );
                                break;
                            }
                        }
                    }

                    if all_ok {
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
                                Some(&active_branding),
                                &shared,
                                &app_handle,
                            );
                            pending_branding_sync = false;
                            last_market_poll = Instant::now();
                        }
                        let _ = fs::remove_file(&xml_path);
                    } else {
                        break;
                    }
                }
                Ok(IncomingPacket::ContractActions(action_list)) => {
                    if !contracts_supported {
                        synthesize_rejected_contract_actions(
                            &paths,
                            &action_list,
                            "CONTRACTS_NOT_SUPPORTED",
                            &last_synced_contract_state,
                            &mut delivered_action_ids,
                        );
                        let _ = fs::remove_file(&xml_path);
                        continue;
                    }

                    let mut all_ok = true;
                    let ui_lang = active_ui_language(&shared);

                    for req in action_list {
                        let req_sid = req.server_id.trim().to_string();
                        if !req_sid.is_empty() && active_server_id.as_deref() != Some(req_sid.as_str())
                        {
                            active_server_id = Some(req_sid.clone());
                            delivered_action_ids.clear();
                            announced_settlement_ids.clear();
                            has_synced_contracts_once = false;
                            if let Ok(mut guard) = shared.lock() {
                                guard.last_seen_server_id = Some(req_sid);
                            }
                        }

                        if req.sub_action == ContractSubAction::RequestSync {
                            delivered_action_ids.clear();
                        }

                        let detail_label = format_contract_action_detail(
                            &req,
                            &known_titles,
                            whitelist_lang.as_deref(),
                            &ui_lang,
                        );
                        let sub_str = req.sub_action.as_str().to_string();
                        let is_user_facing = !matches!(
                            req.sub_action,
                            ContractSubAction::RequestSync | ContractSubAction::AckSettlement
                        );

                        match backend.submit_contract_action(req.clone()) {
                            Ok(state) => {
                                if is_user_facing {
                                    let matching_res = state
                                        .action_results
                                        .iter()
                                        .find(|r| r.action_id == req.action_id);
                                    if let Some(res) = matching_res.filter(|r| r.status == "REJECTED") {
                                        let reason_str = if res.reason.trim().is_empty() {
                                            "REJECTED".to_string()
                                        } else {
                                            res.reason.trim().to_string()
                                        };
                                        push_i18n_log_shared(
                                            &shared,
                                            &app_handle,
                                            LogLevel::Warn,
                                            "logContractActionRejected",
                                            [
                                                ("subAction", sub_str.clone()),
                                                ("reason", reason_str.clone()),
                                            ],
                                            format!(
                                                "Contract action rejected: {sub_str} ({reason_str})"
                                            ),
                                        );
                                    } else {
                                        push_i18n_log_shared(
                                            &shared,
                                            &app_handle,
                                            LogLevel::Success,
                                            "logContractActionSent",
                                            [
                                                ("subAction", sub_str.clone()),
                                                ("detail", detail_label.clone()),
                                            ],
                                            format!(
                                                "Contract action synced: {sub_str} ({detail_label})"
                                            ),
                                        );
                                    }
                                }

                                apply_and_write_contract_state(
                                    &paths,
                                    &state,
                                    true,
                                    &mut has_synced_contracts_once,
                                    &mut last_synced_contract_state,
                                    &mut last_contract_write_at,
                                    &mut delivered_action_ids,
                                    &mut announced_settlement_ids,
                                    &shared,
                                    &app_handle,
                                );
                                last_contract_poll = Instant::now();
                            }
                            Err(ApiError::Validation(reason)) => {
                                synthesize_rejected_contract_actions(
                                    &paths,
                                    std::slice::from_ref(&req),
                                    &reason,
                                    &last_synced_contract_state,
                                    &mut delivered_action_ids,
                                );
                                push_i18n_log_shared(
                                    &shared,
                                    &app_handle,
                                    LogLevel::Warn,
                                    "logContractActionRejected",
                                    [
                                        ("subAction", sub_str.clone()),
                                        ("reason", reason.clone()),
                                    ],
                                    format!("Contract action rejected: {sub_str} ({reason})"),
                                );
                            }
                            Err(ApiError::NotSupported(_)) => {
                                contracts_supported = false;
                                synthesize_rejected_contract_actions(
                                    &paths,
                                    std::slice::from_ref(&req),
                                    "CONTRACTS_NOT_SUPPORTED",
                                    &last_synced_contract_state,
                                    &mut delivered_action_ids,
                                );
                                emit_snapshot_update(&shared, &app_handle, |snap| {
                                    snap.contracts_supported = false;
                                    snap.open_contracts_count = 0;
                                    snap.open_offers_count = 0;
                                });
                                last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
                            }
                            Err(err) => {
                                all_ok = false;
                                handle_backend_io_error(
                                    &err,
                                    &mut is_connected,
                                    &mut retry_hold_until,
                                    &shared,
                                    &app_handle,
                                );
                                break;
                            }
                        }
                    }

                    if all_ok {
                        let _ = fs::remove_file(&xml_path);
                    } else {
                        break;
                    }
                }
                Ok(IncomingPacket::SelectMarket(profile_id)) => {
                    let _ = fs::remove_file(&xml_path);
                    handle_ingame_market_switch(&shared, &app_handle, profile_id);
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
        if stop_mode.load(Ordering::SeqCst) != STOP_RUNNING {
            break;
        }
        if is_connected
            && (force_sync
                || (pending_branding_sync && !last_synced_items.is_empty())
                || last_market_poll.elapsed() >= poll_interval)
        {
            last_market_poll = Instant::now();
            let market_res = backend.fetch_market_state();
            if stop_mode.load(Ordering::SeqCst) != STOP_RUNNING {
                break;
            }
            match market_res {
                Ok(state) => {
                    apply_and_write_market_state(
                        &paths,
                        &state.items,
                        force_sync || pending_branding_sync,
                        &known_titles,
                        whitelist_lang.as_deref(),
                        &mut last_synced_items,
                        &mut known_great_demands,
                        &mut initialized_gd_baseline,
                        Some(&active_branding),
                        &shared,
                        &app_handle,
                    );
                    pending_branding_sync = false;
                }
                Err(err) => {
                    handle_backend_io_error(
                        &err,
                        &mut is_connected,
                        &mut retry_hold_until,
                        &shared,
                        &app_handle,
                    );
                }
            }
        }

        // 5. Periodic Stage 2 contract & direct-offer state poll (`GET /api/v1/contracts/state`).
        if stop_mode.load(Ordering::SeqCst) != STOP_RUNNING {
            break;
        }
        if is_connected
            && contracts_supported
            && (force_sync
                || !has_synced_contracts_once
                || last_contract_poll.elapsed() >= poll_interval)
        {
            last_contract_poll = Instant::now();
            let contract_res = backend.fetch_contracts_state(active_server_id.as_deref());
            if stop_mode.load(Ordering::SeqCst) != STOP_RUNNING {
                break;
            }
            match contract_res {
                Ok(mut state) => {
                    state
                        .action_results
                        .retain(|r| !delivered_action_ids.iter().any(|id| id == &r.action_id));
                    apply_and_write_contract_state(
                        &paths,
                        &state,
                        force_sync || !has_synced_contracts_once,
                        &mut has_synced_contracts_once,
                        &mut last_synced_contract_state,
                        &mut last_contract_write_at,
                        &mut delivered_action_ids,
                        &mut announced_settlement_ids,
                        &shared,
                        &app_handle,
                    );
                }
                Err(ApiError::NotSupported(_)) => {
                    contracts_supported = false;
                    emit_snapshot_update(&shared, &app_handle, |snap| {
                        snap.contracts_supported = false;
                        snap.open_contracts_count = 0;
                        snap.open_offers_count = 0;
                    });
                    last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
                }
                Err(err) => {
                    handle_backend_io_error(
                        &err,
                        &mut is_connected,
                        &mut retry_hold_until,
                        &shared,
                        &app_handle,
                    );
                }
            }
        }
    }

    // Preserve `globalMarketOnline.xml` across live restart handovers (`STOP_RESTART_PRESERVE_HEARTBEAT`)
    // so the new worker can overwrite it atomically; delete it immediately on real shutdown.
    if stop_mode.load(Ordering::SeqCst) == STOP_RESTART_PRESERVE_HEARTBEAT {
        cleanup_bridge_transfer_files(&paths.read_dir, &paths.staging_dir);
    } else {
        cleanup_bridge_runtime_files(&paths.online_xml_file, &paths.read_dir, &paths.staging_dir);
    }
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
    branding: Option<&HeartbeatBranding>,
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
        match write_market_data_xml_with_branding(&paths.staging_dir, &paths.read_dir, items, branding)
        {
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
        .map(|wl| crate::config::languages_equivalent(wl, active_lang))
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
        ApiError::ClientIpLocked { retry_after_sec } => {
            let minutes = retry_after_sec.div_ceil(60).max(1);
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Error,
                "logClientIpLocked",
                [
                    ("minutes", minutes.to_string()),
                    ("seconds", retry_after_sec.to_string()),
                ],
                format!(
                    "Network lock active: This installation was used from another network within the last hour. Automatic retry in ~{minutes} min."
                ),
            );
        }
        ApiError::ClientIdRequired => {
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Error,
                "logClientIdRequired",
                [],
                "Connection rejected: Missing or invalid client identifier (X-Client-Id)."
                    .to_string(),
            );
        }
        ApiError::Unauthorized => {
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Error,
                "logTokenInvalid",
                [],
                "Community/Logo key is invalid or revoked (HTTP 401) – please check Settings."
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
    retry_hold_until: &mut Option<Instant>,
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) {
    *is_connected = false;
    if let ApiError::ClientIpLocked { retry_after_sec } = err {
        *retry_hold_until =
            Some(Instant::now() + Duration::from_secs((*retry_after_sec).clamp(1, 86_400)));
    }
    let msg = err.to_string();
    emit_classified_backend_error(err, LogLevel::Error, shared, app_handle);
    emit_snapshot_update(shared, app_handle, |snap| {
        snap.state = BridgeConnectionState::BackendOffline;
        snap.last_error = Some(msg);
    });
}

/// Minimum interval between two accepted in-game market switch requests (Spec Section 3.1.2).
const INGAME_MARKET_SWITCH_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// Saved market profiles for the in-game market switch heartbeat block (Live mode only; in Mock mode
/// a profile switch does not reconnect, so the mod gets no profiles and hides its button).
fn heartbeat_market_profiles(shared: &Arc<Mutex<SharedState>>) -> Option<HeartbeatMarketProfiles> {
    let guard = shared.lock().ok()?;
    if guard.config.backend_mode != BackendMode::Live {
        return None;
    }
    let profiles = guard
        .token_vault
        .list_profiles()
        .into_iter()
        .map(|p| HeartbeatMarketProfile {
            market_name: p.detected_market_name.filter(|_| p.is_private_market),
            id: p.id,
            label: p.custom_label,
            branding_tier: p.branding_tier,
        })
        .collect();
    Some(HeartbeatMarketProfiles {
        active_id: guard.config.active_token_id.clone(),
        profiles,
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Formats UTC unix seconds as `YYYY-MM-DD HH:MM UTC` for English fallback log texts and diagnostics
/// (the UI formats the `*Unix` log parameters in local time).
pub fn format_unix_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute) = (rem / 3600, (rem % 3600) / 60);
    // Civil-from-days conversion (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// Converts a server notice into the `<serverNotice>` heartbeat element (Spec Section 3.1.3).
fn heartbeat_notice(notice: &ServerNotice, langs: &[String]) -> HeartbeatNotice {
    let lang_refs: Vec<&str> = langs.iter().map(String::as_str).collect();
    HeartbeatNotice {
        id: notice.id.clone(),
        notice_type: notice.notice_type.clone(),
        starts_at: notice.starts_at,
        ends_at: notice.display_end(),
        message: notice.message_for(&lang_refs),
    }
}

/// Stores `next` as the current server notice (memory, `server_notice.json`, snapshot). Announces a
/// notice id seen for the first time (log entry + `bridge://server-notice` event for the toast and the
/// OS notification). Returns `true` when the notice changed.
fn apply_server_notice(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
    current: &mut Option<ServerNotice>,
    next: Option<ServerNotice>,
) -> bool {
    if *current == next {
        return false;
    }
    *current = next.clone();

    let mut announce: Option<ServerNotice> = None;
    let mut ui_lang = "en".to_string();
    if let Ok(mut guard) = shared.lock() {
        guard.server_notice.notice = next.clone();
        if let Some(ref notice) = next {
            if guard.server_notice.notified_id.as_deref() != Some(notice.id.as_str()) {
                guard.server_notice.notified_id = Some(notice.id.clone());
                announce = Some(notice.clone());
            }
        }
        let notice_file = guard.notice_file.clone();
        let _ = guard.server_notice.save(&notice_file);
        ui_lang = guard.config.language.clone();
    }
    emit_snapshot_update(shared, app_handle, |snap| snap.server_notice = next);

    if let Some(notice) = announce {
        log_server_notice(shared, app_handle, &notice, &ui_lang);
        if let Some(app) = app_handle {
            let _ = app.emit("bridge://server-notice", &notice);
        }
    }
    true
}

/// Writes the Activity Log entry for a newly announced server notice.
fn log_server_notice(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
    notice: &ServerNotice,
    ui_lang: &str,
) {
    let langs = message_languages(None, ui_lang);
    let lang_refs: Vec<&str> = langs.iter().map(String::as_str).collect();
    let text = notice.message_for(&lang_refs);
    let note = text.as_ref().map(|t| format!(" – {t}")).unwrap_or_default();
    let start = notice.starts_at.unwrap_or_default();
    let end = notice.display_end();

    match (notice.notice_type.as_str(), end) {
        ("maintenance", Some(end)) => push_i18n_log_shared(
            shared,
            app_handle,
            LogLevel::Warn,
            "logServerNoticeMaintenance",
            [
                ("startUnix", start.to_string()),
                ("start", format_unix_utc(start)),
                ("endUnix", end.to_string()),
                ("end", format_unix_utc(end)),
                ("note", note.clone()),
            ],
            format!(
                "Planned maintenance announced: {} – {}.{note}",
                format_unix_utc(start),
                format_unix_utc(end)
            ),
        ),
        ("maintenance", None) => push_i18n_log_shared(
            shared,
            app_handle,
            LogLevel::Warn,
            "logServerNoticeMaintenanceNoEnd",
            [
                ("startUnix", start.to_string()),
                ("start", format_unix_utc(start)),
                ("note", note.clone()),
            ],
            format!(
                "Planned maintenance announced from {}.{note}",
                format_unix_utc(start)
            ),
        ),
        ("restart", _) => push_i18n_log_shared(
            shared,
            app_handle,
            LogLevel::Warn,
            "logServerNoticeRestart",
            [
                ("startUnix", start.to_string()),
                ("start", format_unix_utc(start)),
                ("note", note.clone()),
            ],
            format!(
                "Short server restart announced: {}.{note}",
                format_unix_utc(start)
            ),
        ),
        _ => {
            let text = text.unwrap_or_else(|| "–".to_string());
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Info,
                "logServerNoticeInfo",
                [("text", text.clone())],
                format!("Notice from the server: {text}"),
            )
        }
    }
}

/// Writes the single Activity Log entry when an outage is classified as maintenance.
fn log_maintenance_started(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
    notice: Option<&ServerNotice>,
) {
    match notice.and_then(ServerNotice::display_end) {
        Some(end) => push_i18n_log_shared(
            shared,
            app_handle,
            LogLevel::Warn,
            "logServerMaintenanceActive",
            [
                ("endUnix", end.to_string()),
                ("end", format_unix_utc(end)),
            ],
            format!(
                "Server maintenance in progress, expected end: {}. The client reconnects automatically.",
                format_unix_utc(end)
            ),
        ),
        None => push_i18n_log_shared(
            shared,
            app_handle,
            LogLevel::Warn,
            "logServerMaintenanceActiveNoEnd",
            [],
            "Server maintenance in progress. The client reconnects automatically.".to_string(),
        ),
    }
}

/// Display text of a profile for log entries (label, market name or masked hint; never the token).
fn market_profile_display(profile: &SavedTokenProfile) -> String {
    profile
        .custom_label
        .clone()
        .or_else(|| profile.detected_market_name.clone())
        .unwrap_or_else(|| profile.masked_hint.clone())
}

/// Handles `write/marketSwitch_*.xml` (`action="selectMarket"`): validates the profile id against the
/// vault, rate-limits to one switch per 30 s and runs [`BridgeManager::select_active_market_token`]
/// on its own thread, because the switch restarts (and joins) this worker.
fn handle_ingame_market_switch(
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
    profile_id: Option<String>,
) {
    let requested_label = {
        let Ok(mut guard) = shared.lock() else {
            return;
        };
        if guard.config.backend_mode != BackendMode::Live {
            drop(guard);
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Warn,
                "logIngameMarketSwitchUnavailable",
                [],
                "Ignored market switch from the game: only available with the live backend."
                    .to_string(),
            );
            return;
        }
        let label = match profile_id.as_deref() {
            Some(id) => match guard.token_vault.get_profile(id) {
                Some(profile) => market_profile_display(&profile),
                None => {
                    drop(guard);
                    push_i18n_log_shared(
                        shared,
                        app_handle,
                        LogLevel::Warn,
                        "logIngameMarketSwitchUnknownProfile",
                        [("profile", id.to_string())],
                        format!("Ignored market switch from the game: unknown profile '{id}'."),
                    );
                    return;
                }
            },
            None => "Global Market".to_string(),
        };
        if guard.config.active_token_id == profile_id {
            return;
        }
        if guard
            .last_ingame_market_switch
            .is_some_and(|last| last.elapsed() < INGAME_MARKET_SWITCH_MIN_INTERVAL)
        {
            drop(guard);
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Warn,
                "logIngameMarketSwitchRateLimited",
                [],
                "Ignored market switch from the game: only one switch per 30 seconds.".to_string(),
            );
            return;
        }
        guard.last_ingame_market_switch = Some(Instant::now());
        label
    };

    push_i18n_log_shared(
        shared,
        app_handle,
        LogLevel::Info,
        "logIngameMarketSwitchRequested",
        [("profile", requested_label.clone())],
        format!("Market switch requested from the game: {requested_label}"),
    );

    let Some(app) = app_handle.clone() else {
        return;
    };
    let shared_clone = Arc::clone(shared);
    let app_for_log = app_handle.clone();
    thread::spawn(move || {
        let Some(manager) = app.try_state::<BridgeManager>() else {
            return;
        };
        if let Err(err) = manager.select_active_market_token(profile_id) {
            push_raw_log_shared(&shared_clone, &app_for_log, LogLevel::Error, err);
        }
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

    // Prioritize `whitelist_*.xml`, then `contractAction_*.xml`, then `transaction_*.xml`, and handle
    // `marketSwitch_*.xml` last so actions written before the switch still reach the previous market.
    // Numeric sequence suffixes sort numerically so `_2.xml` always executes before `_10.xml`.
    files.sort_by(|a, b| {
        let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        write_filename_sort_key(a_name).cmp(&write_filename_sort_key(b_name))
    });

    files
}

fn write_filename_sort_key(filename: &str) -> (u8, String, u64, String) {
    let priority = if filename.starts_with("whitelist_") {
        0
    } else if filename.starts_with("contractAction_") {
        1
    } else if filename.starts_with("transaction_") {
        2
    } else if filename.starts_with("marketSwitch_") {
        4
    } else {
        3
    };
    let stem = filename
        .strip_suffix(".xml")
        .or_else(|| filename.strip_suffix(".XML"))
        .unwrap_or(filename);
    if let Some((prefix_ts, seq_raw)) = stem.rsplit_once('_') {
        if let Ok(seq_num) = seq_raw.parse::<u64>() {
            return (
                priority,
                prefix_ts.to_string(),
                seq_num,
                filename.to_string(),
            );
        }
    }
    (priority, stem.to_string(), 0, filename.to_string())
}

#[allow(clippy::too_many_arguments)]
fn apply_and_write_contract_state(
    paths: &BridgePaths,
    state: &ContractStateResponse,
    force_write_xml: bool,
    has_synced_contracts_once: &mut bool,
    last_synced_contract_state: &mut ContractStateResponse,
    last_contract_write_at: &mut Instant,
    delivered_action_ids: &mut VecDeque<String>,
    announced_settlement_ids: &mut HashSet<String>,
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) {
    let elapsed_secs = last_contract_write_at.elapsed().as_secs() as i64;
    let unacked_settlement_retry_due = !state.settlements.is_empty()
        && elapsed_secs >= 30
        && !has_unread_contract_sync_xml(&paths.read_dir);

    let should_write = force_write_xml
        || !*has_synced_contracts_once
        || !state.action_results.is_empty()
        || unacked_settlement_retry_due
        || contract_state_semantically_changed(last_synced_contract_state, state, elapsed_secs);

    if should_write {
        match write_contract_data_xml(&paths.staging_dir, &paths.read_dir, state) {
            Ok(_) => {
                for res in &state.action_results {
                    record_delivered_action_id(delivered_action_ids, &res.action_id);
                }
                let mut new_settlements = 0_usize;
                for s in &state.settlements {
                    if announced_settlement_ids.insert(s.settlement_id.clone()) {
                        new_settlements += 1;
                    }
                }
                // Prune announced_settlement_ids that are no longer pending on the server
                if state.settlements.is_empty() && announced_settlement_ids.len() > 256 {
                    announced_settlement_ids.clear();
                }
                if new_settlements > 0 {
                    push_i18n_log_shared(
                        shared,
                        app_handle,
                        LogLevel::Success,
                        "logContractSettlementSynced",
                        [("count", new_settlements.to_string())],
                        format!(
                            "Settlements synced: Delivered {new_settlements} contract/offer settlement(s) to FS."
                        ),
                    );
                }

                let mut stored = state.clone();
                stored.action_results.clear();
                *last_synced_contract_state = stored;
                *last_contract_write_at = Instant::now();
                *has_synced_contracts_once = true;
            }
            Err(err) => {
                push_raw_log_shared(shared, app_handle, LogLevel::Error, err);
                return;
            }
        }
    }

    let (r_cnt, w_cnt) = count_bridge_queues(paths);
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    emit_snapshot_update(shared, app_handle, |snap| {
        snap.contracts_supported = true;
        snap.open_contracts_count = state.contracts.len();
        snap.open_offers_count = state.direct_offers.len();
        snap.read_queue_count = r_cnt;
        snap.write_queue_count = w_cnt;
        snap.last_sync_unix = Some(now_secs);
    });
}

fn contract_state_semantically_changed(
    prev: &ContractStateResponse,
    next: &ContractStateResponse,
    elapsed_secs: i64,
) -> bool {
    if prev.contracts.len() != next.contracts.len()
        || prev.direct_offers.len() != next.direct_offers.len()
        || prev.settlements.len() != next.settlements.len()
    {
        return true;
    }

    for (a, b) in prev.contracts.iter().zip(next.contracts.iter()) {
        let a_factor_bp = (a.base_trend_factor * 10_000.0).round() as i64;
        let b_factor_bp = (b.base_trend_factor * 10_000.0).round() as i64;
        let expected_rem = (a.remaining_seconds - elapsed_secs).max(0);
        if a.remote_id != b.remote_id
            || a.origin != b.origin
            || a.is_npc != b.is_npc
            || a.issuer_server_id != b.issuer_server_id
            || a.issuer_farm_id != b.issuer_farm_id
            || a.issuer_name != b.issuer_name
            || a.contractor_server_id != b.contractor_server_id
            || a.contractor_farm_id != b.contractor_farm_id
            || a.fill_type != b.fill_type
            || a.total_liters != b.total_liters
            || a.delivered_liters != b.delivered_liters
            || a_factor_bp != b_factor_bp
            || a.bonus_pct != b.bonus_pct
            || a.status != b.status
            || (b.remaining_seconds - expected_rem).abs() > 120
        {
            return true;
        }
    }

    for (a, b) in prev.direct_offers.iter().zip(next.direct_offers.iter()) {
        let expected_rem = (a.remaining_seconds - elapsed_secs).max(0);
        if a.remote_id != b.remote_id
            || a.origin != b.origin
            || a.is_npc != b.is_npc
            || a.seller_server_id != b.seller_server_id
            || a.seller_farm_id != b.seller_farm_id
            || a.seller_name != b.seller_name
            || a.fill_type != b.fill_type
            || a.total_liters != b.total_liters
            || a.remaining_liters != b.remaining_liters
            || a.price_delta_pct != b.price_delta_pct
            || a.allow_partial != b.allow_partial
            || a.was_bale != b.was_bale
            || a.was_pallet != b.was_pallet
            || a.status != b.status
            || (b.remaining_seconds - expected_rem).abs() > 120
        {
            return true;
        }
    }

    for (a, b) in prev.settlements.iter().zip(next.settlements.iter()) {
        if a.settlement_id != b.settlement_id
            || a.settlement_type != b.settlement_type
            || a.liters != b.liters
            || a.missing_liters != b.missing_liters
        {
            return true;
        }
    }

    false
}

fn record_delivered_action_id(queue: &mut VecDeque<String>, action_id: &str) {
    let clean = action_id.trim();
    if clean.is_empty() || queue.iter().any(|id| id == clean) {
        return;
    }
    if queue.len() >= 512 {
        queue.pop_front();
    }
    queue.push_back(clean.to_string());
}

fn synthesize_rejected_contract_actions(
    paths: &BridgePaths,
    actions: &[ContractActionRequest],
    reason: &str,
    board_baseline: &ContractStateResponse,
    delivered_action_ids: &mut VecDeque<String>,
) {
    let mut results = Vec::new();
    for act in actions {
        if matches!(
            act.sub_action,
            ContractSubAction::RequestSync | ContractSubAction::AckSettlement
        ) {
            continue;
        }
        record_delivered_action_id(delivered_action_ids, &act.action_id);
        results.push(ContractActionResultDto {
            action_id: act.action_id.clone(),
            sub_action: act.sub_action.as_str().to_string(),
            status: "REJECTED".to_string(),
            reason: reason.to_string(),
            remote_id: act.remote_id.clone().unwrap_or_default(),
            granted_liters: 0,
            base_trend_factor: act.base_trend_factor.unwrap_or(1.0),
            bonus_pct: act.bonus_pct.unwrap_or(0),
            remaining_seconds: 0,
        });
    }
    if results.is_empty() {
        return;
    }
    let mut synthetic = board_baseline.clone();
    synthetic.action_results = results;
    let _ = write_contract_data_xml(&paths.staging_dir, &paths.read_dir, &synthetic);
}

fn format_contract_action_detail(
    req: &ContractActionRequest,
    known_titles: &HashMap<String, String>,
    whitelist_lang: Option<&str>,
    ui_lang: &str,
) -> String {
    if let Some(ref ft) = req.fill_type {
        let title = resolve_fill_type_code(ft, None, known_titles, whitelist_lang, ui_lang);
        if let Some(liters) = req.liters {
            return format!("{liters} L {title}");
        }
        return title;
    }
    if let Some(ref rid) = req.remote_id {
        if let Some(liters) = req.liters {
            return format!("#{rid} • {liters} L");
        }
        return format!("#{rid}");
    }
    req.action_id.clone()
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
            guard.snapshot.extended_expires_unix = None;
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

/// Resolves or downloads & converts the active custom market logo into `logos/logo_<12char_sha256>.dds`.
/// Skips network download and DDS re-encoding whenever the deterministic cached `.dds` file already exists.
fn resolve_or_cache_handshake_logo(
    paths: &BridgePaths,
    backend: &dyn BackendService,
    hs: &ServerHandshake,
    failed_logo_attempts: &mut HashMap<String, Instant>,
    shared: &Arc<Mutex<SharedState>>,
    app_handle: &Option<AppHandle>,
) -> Option<String> {
    if !matches!(hs.branding_tier, 1 | 3) {
        return None;
    }
    let logo_url = hs.market_logo_url.as_deref()?.trim();
    if logo_url.is_empty() {
        return None;
    }

    let hash12 = logo_cache_hash12(logo_url.as_bytes());
    let rel_logo = logo_rel_path_for_hash(&hash12);
    let target_dds = paths.logos_dir.join(format!("logo_{hash12}.dds"));

    if is_valid_cached_logo_dds(&target_dds) {
        failed_logo_attempts.remove(logo_url);
        return Some(rel_logo);
    }

    if let Some(last_fail) = failed_logo_attempts.get(logo_url) {
        if last_fail.elapsed() < Duration::from_secs(30) {
            return None;
        }
    }

    let raw_bytes = match backend.fetch_logo_bytes(logo_url) {
        Ok(bytes) => bytes,
        Err(err) => {
            failed_logo_attempts.insert(logo_url.to_string(), Instant::now());
            let err_str = err.to_string();
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Warn,
                "logLogoDownloadFailed",
                [("error", err_str.clone())],
                format!("Failed to download custom market logo: {err_str}"),
            );
            return None;
        }
    };

    match convert_logo_bytes_to_dds_file(&raw_bytes, &paths.staging_dir, &target_dds) {
        Ok(()) => {
            failed_logo_attempts.remove(logo_url);
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Success,
                "logLogoCached",
                [("file", rel_logo.clone())],
                format!("Custom market logo cached as '{rel_logo}' (512x256 DXT5)."),
            );
            Some(rel_logo)
        }
        Err(conv_err) => {
            failed_logo_attempts.insert(logo_url.to_string(), Instant::now());
            push_i18n_log_shared(
                shared,
                app_handle,
                LogLevel::Warn,
                "logLogoConvertFailed",
                [("error", conv_err.clone())],
                format!("Failed to convert custom market logo to DDS: {conv_err}"),
            );
            None
        }
    }
}

/// Loads and validates a local `.webp` / `.png` / `.jpg` / `.dds` image completely in pure Rust
/// (no external CLI tools required), returning both a WebView `data:` preview URL and raw bytes for DDS conversion.
fn load_local_logo_with_bytes(file_path: &Path) -> Result<(String, Vec<u8>), String> {
    if !file_path.is_file() {
        return Err(format!("Logo file not found: {}", file_path.display()));
    }

    let meta = fs::metadata(file_path).map_err(|e| format!("Cannot inspect logo file: {e}"))?;
    if meta.len() > MAX_LOGO_RAW_BYTES as u64 {
        return Err("Logo file exceeds 8 MB limit.".to_string());
    }

    let bytes = fs::read(file_path).map_err(|e| format!("Cannot read logo file: {e}"))?;
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let decoded = decode_logo_rgba8(&bytes)?;

    if ext == "dds" || (bytes.len() >= 4 && &bytes[0..4] == b"DDS ") {
        let png_bytes = encode_rgba8_to_png_bytes(&decoded)?;
        let data_url = format!("data:image/png;base64,{}", encode_base64(&png_bytes));
        return Ok((data_url, bytes));
    }

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
                "Unsupported logo format. Please select a WebP, PNG, JPG, or DDS file.".to_string(),
            );
        }
    };

    let data_url = format!("data:{mime};base64,{}", encode_base64(&bytes));
    Ok((data_url, bytes))
}
