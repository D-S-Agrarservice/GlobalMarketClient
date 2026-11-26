pub mod api;
pub mod bridge;
pub mod commands;
pub mod config;
pub mod diagnostics;
pub mod tray;

use crate::bridge::coordinator::BridgeManager;
use crate::tray::AppExitState;
use tauri::{Manager, RunEvent, WindowEvent};

/// Standard 3-part SemVer protocol version (`MAJOR.MINOR.PATCH`) reported in `globalMarketOnline.xml`.
pub const PROTOCOL_EXE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Flags pre-release Beta builds (`0.9.x`) across UI badges, window titles, and diagnostic reports
/// while keeping `Cargo.toml` / `tauri.conf.json` strictly numeric (`0.9.11`) for Windows MSI/NSIS & Lua SemVer parsing.
pub const IS_BETA_BUILD: bool = true;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppExitState::default())
        .setup(|app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("globalmarket-client"));
            let config_file = config_dir.join("settings.json");

            let manager = BridgeManager::new(config_file, Some(app.handle().clone()));
            let initial_cfg = manager.get_config();
            let initial_snap = manager.get_snapshot();
            app.manage(manager);

            tray::setup_system_tray(
                app,
                &initial_cfg.language,
                initial_snap.game_edition.as_deref(),
                initial_snap.connection_enabled,
            )?;
            tray::apply_window_theme(app.handle(), &initial_cfg.theme);
            tray::restore_main_window(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app_handle = window.app_handle();
                let is_quitting = app_handle
                    .try_state::<AppExitState>()
                    .map(|s| s.is_quitting())
                    .unwrap_or(false);

                if is_quitting {
                    return;
                }

                let minimize_to_tray = app_handle
                    .try_state::<BridgeManager>()
                    .map(|m| m.get_config().minimize_to_tray)
                    .unwrap_or(true);

                if minimize_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    api.prevent_close();
                    tray::quit_application(app_handle);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_info,
            commands::get_bridge_snapshot,
            commands::set_bridge_connected,
            commands::get_bridge_logs,
            commands::get_app_config,
            commands::set_fs25_profile_path,
            commands::update_app_config,
            commands::preview_window_theme,
            commands::add_market_token,
            commands::set_dev_token,
            commands::select_active_market_token,
            commands::update_market_token_label,
            commands::remove_market_token,
            commands::unlock_access_tier,
            commands::reset_access_tier,
            commands::generate_diagnostic_report,
            commands::quit_app,
            commands::prepare_for_update,
            commands::mock_trigger_great_demand,
            commands::mock_approve_pending,
            commands::mock_set_offline,
            commands::mock_set_min_version,
            commands::mock_set_latest_version,
            commands::mock_set_server_notice_stage,
            commands::mock_set_community_logo,
            commands::mock_set_branding_tier,
            commands::mock_simulate_contract_activity,
        ])
        .build(tauri::generate_context!())
        .expect("error while building GlobalMarketClient application");

    app.run(|app_handle, event| {
        if let RunEvent::Exit = event {
            if let Some(manager) = app_handle.try_state::<BridgeManager>() {
                manager.shutdown();
            }
        }
    });
}
