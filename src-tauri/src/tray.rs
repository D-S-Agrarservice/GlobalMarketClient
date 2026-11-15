//! System tray integration, localized context menu, window close-to-tray handling,
//! and native OS desktop notifications (works reliably even while hidden in tray).

use crate::bridge::coordinator::BridgeManager;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Wry};
use tauri_plugin_notification::NotificationExt;

pub const TRAY_ID: &str = "main-tray";
const MENU_SHOW_ID: &str = "tray_show";
const MENU_TOGGLE_CONN_ID: &str = "tray_toggle_connection";
const MENU_QUIT_ID: &str = "tray_quit";

/// Tracks whether an explicit quit was requested (via Tray menu or UI Quit button)
/// so the window `CloseRequested` interceptor never blocks a real shutdown.
#[derive(Default)]
pub struct AppExitState {
    is_quitting: AtomicBool,
}

impl AppExitState {
    pub fn mark_quitting(&self) {
        self.is_quitting.store(true, Ordering::SeqCst);
    }

    pub fn swap_quitting(&self) -> bool {
        self.is_quitting.swap(true, Ordering::SeqCst)
    }

    pub fn is_quitting(&self) -> bool {
        self.is_quitting.load(Ordering::SeqCst)
    }
}

struct TrayLabels {
    show_window: &'static str,
    connect_bridge: &'static str,
    disconnect_bridge: &'static str,
    quit_app: &'static str,
}

pub fn format_brand_title(game_edition: Option<&str>) -> String {
    match game_edition.map(str::trim).filter(|s| !s.is_empty()) {
        Some(ed) => format!("{ed} GlobalMarket"),
        None => "GlobalMarket".to_string(),
    }
}

fn format_window_and_tray_title(game_edition: Option<&str>) -> String {
    if crate::IS_BETA_BUILD {
        format!("{} – D-S-Agrarservice (Beta)", format_brand_title(game_edition))
    } else {
        format!("{} – D-S-Agrarservice", format_brand_title(game_edition))
    }
}

fn tray_labels(lang: &str) -> TrayLabels {
    match lang {
        "de" => TrayLabels {
            show_window: "Fenster öffnen",
            connect_bridge: "Mit Markt verbinden",
            disconnect_bridge: "Verbindung trennen",
            quit_app: "GlobalMarket beenden",
        },
        "fr" => TrayLabels {
            show_window: "Ouvrir la fenêtre",
            connect_bridge: "Connecter au marché",
            disconnect_bridge: "Déconnecter",
            quit_app: "Quitter GlobalMarket",
        },
        "es" => TrayLabels {
            show_window: "Abrir ventana",
            connect_bridge: "Conectar al mercado",
            disconnect_bridge: "Desconectar",
            quit_app: "Salir de GlobalMarket",
        },
        "pl" => TrayLabels {
            show_window: "Otwórz okno",
            connect_bridge: "Połącz z rynkiem",
            disconnect_bridge: "Rozłącz",
            quit_app: "Zakończ GlobalMarket",
        },
        "it" => TrayLabels {
            show_window: "Apri finestra",
            connect_bridge: "Connetti al mercato",
            disconnect_bridge: "Disconnetti",
            quit_app: "Esci da GlobalMarket",
        },
        "nl" => TrayLabels {
            show_window: "Venster openen",
            connect_bridge: "Verbinden met markt",
            disconnect_bridge: "Verbinding verbreken",
            quit_app: "GlobalMarket afsluiten",
        },
        "cz" | "cs" => TrayLabels {
            show_window: "Otevřít okno",
            connect_bridge: "Připojit k trhu",
            disconnect_bridge: "Odpojit",
            quit_app: "Ukončit GlobalMarket",
        },
        "pt" | "br" => TrayLabels {
            show_window: "Abrir janela",
            connect_bridge: "Conectar ao mercado",
            disconnect_bridge: "Desconectar",
            quit_app: "Sair do GlobalMarket",
        },
        "hu" => TrayLabels {
            show_window: "Ablak megnyitása",
            connect_bridge: "Csatlakozás a piachoz",
            disconnect_bridge: "Leválasztás",
            quit_app: "Kilépés a GlobalMarketből",
        },
        "da" | "dk" => TrayLabels {
            show_window: "Åbn vindue",
            connect_bridge: "Forbind til marked",
            disconnect_bridge: "Afbryd forbindelse",
            quit_app: "Afslut GlobalMarket",
        },
        "sv" | "se" => TrayLabels {
            show_window: "Öppna fönster",
            connect_bridge: "Anslut till marknad",
            disconnect_bridge: "Koppla från",
            quit_app: "Avsluta GlobalMarket",
        },
        "ro" => TrayLabels {
            show_window: "Deschide fereastra",
            connect_bridge: "Conectare la piață",
            disconnect_bridge: "Deconectare",
            quit_app: "Ieșire din GlobalMarket",
        },
        "tr" => TrayLabels {
            show_window: "Pencereyi Aç",
            connect_bridge: "Pazara Bağlan",
            disconnect_bridge: "Bağlantıyı Kes",
            quit_app: "GlobalMarket'ten Çık",
        },
        "ru" => TrayLabels {
            show_window: "Открыть окно",
            connect_bridge: "Подключиться к рынку",
            disconnect_bridge: "Отключиться",
            quit_app: "Выйти из GlobalMarket",
        },
        "uk" | "ua" => TrayLabels {
            show_window: "Відкрити вікно",
            connect_bridge: "Підключитися до ринку",
            disconnect_bridge: "Відключитися",
            quit_app: "Вийти з GlobalMarket",
        },
        _ => TrayLabels {
            show_window: "Open Window",
            connect_bridge: "Connect to Market",
            disconnect_bridge: "Disconnect",
            quit_app: "Quit GlobalMarket",
        },
    }
}

fn build_tray_menu(
    handle: &AppHandle<Wry>,
    lang: &str,
    connection_enabled: bool,
) -> tauri::Result<Menu<Wry>> {
    let labels = tray_labels(lang);
    let show_item = MenuItem::with_id(handle, MENU_SHOW_ID, labels.show_window, true, None::<&str>)?;
    let toggle_label = if connection_enabled {
        labels.disconnect_bridge
    } else {
        labels.connect_bridge
    };
    let toggle_item =
        MenuItem::with_id(handle, MENU_TOGGLE_CONN_ID, toggle_label, true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(handle)?;
    let quit_item = MenuItem::with_id(handle, MENU_QUIT_ID, labels.quit_app, true, None::<&str>)?;

    Menu::with_items(handle, &[&show_item, &toggle_item, &separator, &quit_item])
}

/// Initializes the system tray icon and context menu at startup.
pub fn setup_system_tray(
    app: &App,
    initial_lang: &str,
    game_edition: Option<&str>,
    connection_enabled: bool,
) -> tauri::Result<()> {
    let handle = app.handle();
    let tooltip = format_window_and_tray_title(game_edition);
    let menu = build_tray_menu(handle, initial_lang, connection_enabled)?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(&tooltip)
        .show_menu_on_left_click(false)
        .on_menu_event(|app_handle, event| match event.id().as_ref() {
            MENU_SHOW_ID => restore_main_window(app_handle),
            MENU_TOGGLE_CONN_ID => {
                // Run worker teardown/restart off the Windows tray event loop to avoid blocking UI messages.
                let handle_clone = app_handle.clone();
                std::thread::spawn(move || {
                    if let Some(manager) = handle_clone.try_state::<BridgeManager>() {
                        let next = !manager.is_connection_enabled();
                        manager.set_connection_enabled(next);
                    }
                });
            }
            MENU_QUIT_ID => quit_application(app_handle),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                restore_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    sync_window_and_tray_edition(handle, game_edition);
    Ok(())
}

/// Refreshes the system tray context menu (language + Connect/Disconnect label) and title/tooltip.
pub fn update_tray_state(
    app_handle: &AppHandle,
    lang: &str,
    game_edition: Option<&str>,
    connection_enabled: bool,
) {
    if let Some(tray) = app_handle.tray_by_id(TRAY_ID) {
        if let Ok(menu) = build_tray_menu(app_handle, lang, connection_enabled) {
            let _ = tray.set_menu(Some(menu));
        }
    }
    sync_window_and_tray_edition(app_handle, game_edition);
}

/// Refreshes the system tray menu labels dynamically when the user switches UI language.
pub fn update_tray_language(app_handle: &AppHandle, lang: &str, game_edition: Option<&str>) {
    let connection_enabled = app_handle
        .try_state::<BridgeManager>()
        .map(|m| m.is_connection_enabled())
        .unwrap_or(true);
    update_tray_state(app_handle, lang, game_edition, connection_enabled);
}

/// Updates both the main window title and system tray tooltip to match the active FS edition (`FS25`, `FS28`, etc.).
pub fn sync_window_and_tray_edition(app_handle: &AppHandle, game_edition: Option<&str>) {
    let full_title = format_window_and_tray_title(game_edition);
    if let Some(tray) = app_handle.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(&full_title));
    }
    if let Some(win) = app_handle.get_webview_window("main") {
        let _ = win.set_title(&full_title);
    }
}

/// Applies the native OS window titlebar theme (`dark` default, `light`, or `None` for OS `system`).
pub fn apply_window_theme(app_handle: &AppHandle, theme: &str) {
    if let Some(win) = app_handle.get_webview_window("main") {
        let target = match crate::config::normalize_theme(theme).as_str() {
            "light" => Some(tauri::Theme::Light),
            "system" => None,
            _ => Some(tauri::Theme::Dark),
        };
        let _ = win.set_theme(target);
    }
}

/// Restores, unminimizes, and focuses the main application window.
pub fn restore_main_window(app_handle: &AppHandle) {
    if let Some(win) = app_handle.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// Immediately cleans up `globalMarketOnline.xml`, removes the tray icon, and exits the process cleanly
/// without blocking the Windows main UI message loop if an HTTP timeout is mid-flight.
pub fn quit_application(app_handle: &AppHandle) {
    if let Some(exit_state) = app_handle.try_state::<AppExitState>() {
        if exit_state.swap_quitting() {
            return;
        }
    }

    let _ = app_handle.remove_tray_by_id(TRAY_ID);
    if let Some(win) = app_handle.get_webview_window("main") {
        let _ = win.hide();
    }

    let pending_worker = app_handle
        .try_state::<BridgeManager>()
        .and_then(|manager| manager.prepare_fast_exit());

    let handle_clone = app_handle.clone();
    std::thread::spawn(move || {
        if let Some(worker) = pending_worker {
            let start = std::time::Instant::now();
            while !worker.is_finished() && start.elapsed() < std::time::Duration::from_millis(400) {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
        handle_clone.exit(0);
    });
}

/// Dispatches a localized native OS desktop notification for a newly started Great Demand.
pub fn send_great_demand_os_notification(
    app_handle: &AppHandle,
    lang: &str,
    game_edition: Option<&str>,
    item_title: &str,
    bonus_percent: u32,
) {
    let brand = format_brand_title(game_edition);
    let (title, body) = match lang {
        "de" => (
            format!("{brand} – Große Nachfrage!"),
            format!("{item_title}: Jetzt +{bonus_percent}% Verkaufsbonus auf dem Weltmarkt!"),
        ),
        "fr" => (
            format!("{brand} – Forte demande !"),
            format!("{item_title} : +{bonus_percent}% de bonus de vente sur le marché mondial !"),
        ),
        "es" => (
            format!("{brand} – ¡Gran demanda!"),
            format!("{item_title}: ¡+{bonus_percent}% de bonificación de venta en el mercado global!"),
        ),
        "pl" => (
            format!("{brand} – Duży popyt!"),
            format!("{item_title}: Teraz +{bonus_percent}% premii za sprzedaż na rynku globalnym!"),
        ),
        "it" => (
            format!("{brand} – Grande richiesta!"),
            format!("{item_title}: Ora +{bonus_percent}% di bonus vendita sul mercato globale!"),
        ),
        "nl" => (
            format!("{brand} – Grote vraag!"),
            format!("{item_title}: Nu +{bonus_percent}% verkoopbonus op de wereldmarkt!"),
        ),
        "cz" | "cs" => (
            format!("{brand} – Vysoká poptávka!"),
            format!("{item_title}: Nyní +{bonus_percent}% prodejní bonus na globálním trhu!"),
        ),
        "pt" | "br" => (
            format!("{brand} – Grande demanda!"),
            format!("{item_title}: Agora +{bonus_percent}% de bônus de venda no mercado global!"),
        ),
        "hu" => (
            format!("{brand} – Magas kereslet!"),
            format!("{item_title}: Most +{bonus_percent}% eladási bónusz a globális piacon!"),
        ),
        "da" | "dk" => (
            format!("{brand} – Stor efterspørgsel!"),
            format!("{item_title}: Nu +{bonus_percent}% salgsbonus på det globale marked!"),
        ),
        "sv" | "se" => (
            format!("{brand} – Stor efterfrågan!"),
            format!("{item_title}: Nu +{bonus_percent}% säljbonus på den globala marknaden!"),
        ),
        "ro" => (
            format!("{brand} – Cerere mare!"),
            format!("{item_title}: Acum +{bonus_percent}% bonus de vânzare pe piața globală!"),
        ),
        "tr" => (
            format!("{brand} – Yüksek Talep!"),
            format!("{item_title}: Küresel pazarda şimdi +%{bonus_percent} satış bonusu!"),
        ),
        "ru" => (
            format!("{brand} – Высокий спрос!"),
            format!("{item_title}: Сейчас +{bonus_percent}% бонус к продаже на глобальном рынке!"),
        ),
        "uk" | "ua" => (
            format!("{brand} – Високий попит!"),
            format!("{item_title}: Зараз +{bonus_percent}% бонус до продажу на глобальному ринку!"),
        ),
        _ => (
            format!("{brand} – Great Demand!"),
            format!("{item_title}: +{bonus_percent}% selling bonus active on the Global Market!"),
        ),
    };

    let _ = app_handle
        .notification()
        .builder()
        .title(&title)
        .body(&body)
        .show();
}
