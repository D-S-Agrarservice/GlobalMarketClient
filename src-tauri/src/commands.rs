//! Tauri IPC command handlers bridging the TypeScript UI with the Rust `BridgeManager`.

use crate::bridge::coordinator::{BridgeManager, BridgeSnapshot, LogEntry};
use crate::config::AppConfig;
use serde::Serialize;
use tauri::{AppHandle, State};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub app_version: &'static str,
    pub protocol_version: &'static str,
    pub is_beta: bool,
    pub platform: &'static str,
}

#[tauri::command]
pub fn get_app_info() -> AppInfo {
    AppInfo {
        app_version: env!("CARGO_PKG_VERSION"),
        protocol_version: crate::PROTOCOL_EXE_VERSION,
        is_beta: crate::IS_BETA_BUILD,
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
pub fn get_bridge_snapshot(bridge: State<'_, BridgeManager>) -> BridgeSnapshot {
    bridge.get_snapshot()
}

#[tauri::command]
pub fn get_bridge_logs(bridge: State<'_, BridgeManager>) -> Vec<LogEntry> {
    bridge.get_logs()
}

#[tauri::command]
pub fn get_app_config(bridge: State<'_, BridgeManager>) -> AppConfig {
    bridge.get_config()
}

#[tauri::command]
pub fn set_fs25_profile_path(
    path: String,
    bridge: State<'_, BridgeManager>,
) -> Result<BridgeSnapshot, String> {
    bridge.set_profile_path(&path)
}

#[tauri::command]
pub fn update_app_config(
    app_handle: AppHandle,
    config: AppConfig,
    bridge: State<'_, BridgeManager>,
) -> Result<BridgeSnapshot, String> {
    let snap = bridge.update_config(config)?;
    let updated_lang = bridge.get_config().language;
    crate::tray::update_tray_language(&app_handle, &updated_lang, snap.game_edition.as_deref());
    Ok(snap)
}

#[tauri::command]
pub fn unlock_access_tier(
    key: String,
    bridge: State<'_, BridgeManager>,
) -> Result<AppConfig, String> {
    bridge.unlock_access_tier(&key)
}

#[tauri::command]
pub fn reset_access_tier(bridge: State<'_, BridgeManager>) -> Result<AppConfig, String> {
    bridge.reset_access_tier()
}

#[tauri::command]
pub fn add_market_token(
    token: String,
    custom_label: Option<String>,
    bridge: State<'_, BridgeManager>,
) -> Result<BridgeSnapshot, String> {
    bridge.add_market_token(&token, custom_label.as_deref())
}

#[tauri::command]
pub fn select_active_market_token(
    token_id: Option<String>,
    bridge: State<'_, BridgeManager>,
) -> Result<BridgeSnapshot, String> {
    bridge.select_active_market_token(token_id)
}

#[tauri::command]
pub fn update_market_token_label(
    token_id: String,
    custom_label: Option<String>,
    bridge: State<'_, BridgeManager>,
) -> Result<BridgeSnapshot, String> {
    bridge.update_market_token_label(&token_id, custom_label.as_deref())
}

#[tauri::command]
pub fn remove_market_token(
    token_id: String,
    bridge: State<'_, BridgeManager>,
) -> Result<BridgeSnapshot, String> {
    bridge.remove_market_token(&token_id)
}

#[tauri::command]
pub fn generate_diagnostic_report(bridge: State<'_, BridgeManager>) -> String {
    bridge.generate_diagnostic_report()
}

#[tauri::command]
pub fn quit_app(app_handle: AppHandle) {
    crate::tray::quit_application(&app_handle);
}

#[tauri::command]
pub fn prepare_for_update(
    app_handle: AppHandle,
    bridge: State<'_, BridgeManager>,
    exit_state: State<'_, crate::tray::AppExitState>,
) {
    exit_state.mark_quitting();
    let _ = app_handle;
    bridge.shutdown();
}

#[tauri::command]
pub fn mock_trigger_great_demand(
    fill_type: Option<String>,
    bridge: State<'_, BridgeManager>,
) -> Option<String> {
    bridge.mock_trigger_great_demand(fill_type)
}

#[tauri::command]
pub fn mock_approve_pending(bridge: State<'_, BridgeManager>) -> usize {
    bridge.mock_approve_pending()
}

#[tauri::command]
pub fn mock_set_offline(offline: bool, bridge: State<'_, BridgeManager>) {
    bridge.mock_set_offline(offline);
}

#[tauri::command]
pub fn mock_set_min_version(min_version: String, bridge: State<'_, BridgeManager>) {
    bridge.mock_set_min_version(min_version);
}

#[tauri::command]
pub fn mock_set_latest_version(latest_version: Option<String>, bridge: State<'_, BridgeManager>) {
    bridge.mock_set_latest_version(latest_version);
}

#[tauri::command]
pub fn mock_set_community_logo(
    path: Option<String>,
    bridge: State<'_, BridgeManager>,
) -> Result<BridgeSnapshot, String> {
    bridge.mock_set_community_logo(path)
}

#[tauri::command]
pub fn mock_set_branding_tier(
    tier: u8,
    market_name: Option<String>,
    bridge: State<'_, BridgeManager>,
) -> Result<BridgeSnapshot, String> {
    bridge.mock_set_branding_tier(tier, market_name)
}
