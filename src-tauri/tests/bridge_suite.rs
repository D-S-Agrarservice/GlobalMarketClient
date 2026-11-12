//! Comprehensive integration & unit test suite for the FS25 GlobalMarket Rust Core (Phases 2–4 & 3.5).

use globalmarket_client_lib::api::fastapi::{resolve_safe_logo_url, FastApiBackend};
use globalmarket_client_lib::api::mock::LocalMockBackend;
use globalmarket_client_lib::api::types::{
    FillTypeStatus, MarketItemDto, TransactionRequest, TransactionType, WhitelistItemDto,
    WhitelistSyncRequest,
};
use globalmarket_client_lib::api::{ApiError, BackendService};
use globalmarket_client_lib::bridge::coordinator::{
    encode_base64, BridgeConnectionState, BridgeManager,
};
use globalmarket_client_lib::bridge::l10n::localize_fill_type;
use globalmarket_client_lib::bridge::paths::resolve_bridge_paths;
use globalmarket_client_lib::bridge::xml::{
    is_version_sufficient, parse_incoming_write_xml, remove_heartbeat_xml, write_heartbeat_xml,
    write_market_data_xml, IncomingPacket, MAX_RETAINED_MARKET_SYNC_FILES,
};
use globalmarket_client_lib::config::{
    digest_access_key, qualify_transaction_id, verify_access_key, verify_access_key_with_digests,
    AccessTier, AppConfig, BackendMode, DEFAULT_OFFICIAL_BACKEND_URL, OFFICIAL_DEV_BACKEND_URL,
    OFFICIAL_LOGO_CDN_URL,
};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[test]
fn writes_heartbeat_and_market_xml_strictly_without_utf8_bom() {
    let dir = env::temp_dir().join(format!("fs25_gm_xml_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let staging = dir.join(".staging");
    let read_dir = dir.join("read");
    let online_file = dir.join("globalMarketOnline.xml");

    write_heartbeat_xml(&staging, &online_file, true, "1.0.0.0", "1.0.0.0").unwrap();
    let online_bytes = fs::read(&online_file).unwrap();
    assert!(
        !online_bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
        "Must never emit UTF-8 BOM!"
    );
    let online_str = String::from_utf8(online_bytes).unwrap();
    assert!(online_str.contains("isConnected=\"true\""));
    assert!(online_str.contains("<version>1.0.0.0</version>"));
    assert!(online_str.contains("<minVersion>1.0.0.0</minVersion>"));

    let written = write_market_data_xml(
        &staging,
        &read_dir,
        &[
            MarketItemDto {
                fill_type: "WHEAT".to_string(),
                title: Some("Weizen".to_string()),
                fill_level: 1_450_000,
                trend: 0.92,
                great_demand: false,
                great_demand_bonus: 0.0,
                great_demand_remaining: 0,
                status: FillTypeStatus::Approved,
            },
            MarketItemDto {
                fill_type: "BARLEY".to_string(),
                title: Some("Gerste".to_string()),
                fill_level: 450_000,
                trend: 1.05,
                great_demand: true,
                great_demand_bonus: 0.25,
                great_demand_remaining: 200_000,
                status: FillTypeStatus::Approved,
            },
        ],
    )
    .unwrap();

    let md_bytes = fs::read(&written).unwrap();
    assert!(!md_bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
    let md_str = String::from_utf8(md_bytes).unwrap();
    assert!(md_str.contains("trend=\"0.92\""));
    assert!(md_str.contains(
        "greatDemand=\"true\" greatDemandBonus=\"0.25\" greatDemandRemaining=\"200000\""
    ));
    assert!(md_str.contains("status=\"APPROVED\""));

    // Writing 8 additional snapshots retains at most MAX_RETAINED_MARKET_SYNC_FILES (3) in read/
    for idx in 1..=8 {
        write_market_data_xml(
            &staging,
            &read_dir,
            &[MarketItemDto {
                fill_type: "WHEAT".to_string(),
                title: Some("Weizen".to_string()),
                fill_level: 1_000_000 + idx * 1_000,
                trend: 1.0,
                great_demand: false,
                great_demand_bonus: 0.0,
                great_demand_remaining: 0,
                status: FillTypeStatus::Approved,
            }],
        )
        .unwrap();
    }
    let retained_files: Vec<PathBuf> = fs::read_dir(&read_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert_eq!(retained_files.len(), MAX_RETAINED_MARKET_SYNC_FILES);
    let newest_present = retained_files
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .any(|s| s.contains("fillLevel=\"1008000\""));
    assert!(newest_present, "Newest gmDataSync snapshot must be retained");

    remove_heartbeat_xml(&online_file);
    assert!(!online_file.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn parses_whitelist_transactions_and_ignores_unknown_future_actions() {
    let wl_xml = r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="updateWhitelist" lang="de">
    <whitelist>
        <item name="WHEAT" title="Weizen" category="GRAINS" type="BULK" basePrice="0.333000" source="BASEGAME" />
        <item name="SPELT" title="Dinkel" category="GRAINS" type="BULK" basePrice="0.360000" source="FS25_MapName" />
    </whitelist>
</gmDataSync>"#;

    match parse_incoming_write_xml(wl_xml, "whitelist_1729512000").unwrap() {
        IncomingPacket::UpdateWhitelist(req) => {
            assert_eq!(req.lang, "de");
            assert_eq!(req.items.len(), 2);
            assert_eq!(req.items[1].name, "SPELT");
            assert_eq!(req.items[1].source, "FS25_MapName");
        }
        other => panic!("unexpected packet: {other:?}"),
    }

    let tx_xml = r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="transaction">
    <entry fillType="WHEAT" amount="50000" type="BUY" />
</gmDataSync>"#;

    match parse_incoming_write_xml(tx_xml, "1729512015_1").unwrap() {
        IncomingPacket::Transactions(list) => {
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].tx_id, "1729512015_1");
            assert_eq!(list[0].fill_type, "WHEAT");
            assert_eq!(list[0].amount, 50_000);
            assert_eq!(list[0].tx_type, TransactionType::Buy);
        }
        other => panic!("unexpected packet: {other:?}"),
    }

    let future_xml = r#"<gmDataSync action="livestockMarket"><animal type="COW"/></gmDataSync>"#;
    assert!(matches!(
        parse_incoming_write_xml(future_xml, "future_1").unwrap(),
        IncomingPacket::IgnoredUnknownAction(act) if act == "livestockMarket"
    ));
}

#[test]
fn version_comparison_matches_lua_semantics() {
    // 3-part standard SemVer and 4-part legacy compatibility
    assert!(is_version_sufficient("1.0.0", "1.0.0"));
    assert!(is_version_sufficient("1.0.0", "1.0.0.0"));
    assert!(is_version_sufficient("1.0.0.0", "1.0.0"));
    assert!(is_version_sufficient("1.0.1", "1.0.0.9"));
    assert!(!is_version_sufficient("1.0.0", "1.0.1"));
    assert!(!is_version_sufficient("1.0.0", "1.0.0.1"));
    assert!(!is_version_sufficient("", "1.0.0"));
}

#[test]
fn resolves_all_three_directory_levels_and_custom_profile() {
    let temp_root = env::temp_dir().join(format!("fs25_gm_path_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_root);

    let custom_profile = temp_root.join("CustomProfileDir");
    fs::create_dir_all(&custom_profile).unwrap();
    fs::write(custom_profile.join("gameSettings.xml"), "<gameSettings/>").unwrap();

    let p1 = resolve_bridge_paths(&custom_profile).expect("resolves custom profile");
    assert_eq!(p1.game_edition, "FS25");
    assert!(p1.bridge_root.ends_with("FS25_GlobalMarket"));
    p1.ensure_directories().unwrap();

    let p2 = resolve_bridge_paths(&custom_profile.join("modSettings")).unwrap();
    assert_eq!(p2.game_edition, "FS25");
    assert_eq!(p1.bridge_root, p2.bridge_root);

    let p3 = resolve_bridge_paths(&p1.bridge_root).unwrap();
    assert_eq!(p3.game_edition, "FS25");
    assert_eq!(p1.bridge_root, p3.bridge_root);

    // Multi-version support: FarmingSimulator2028 resolves dynamically to FS28 & FS28_GlobalMarket
    let fs28_profile = temp_root.join("FarmingSimulator2028");
    fs::create_dir_all(&fs28_profile).unwrap();
    fs::write(fs28_profile.join("gameSettings.xml"), "<gameSettings/>").unwrap();

    let p28 = resolve_bridge_paths(&fs28_profile).expect("resolves FS28 profile");
    assert_eq!(p28.game_edition, "FS28");
    assert!(p28.bridge_root.ends_with("FS28_GlobalMarket"));
    p28.ensure_directories().unwrap();
    assert_eq!(
        resolve_bridge_paths(&p28.bridge_root).unwrap().game_edition,
        "FS28"
    );

    // Custom -profile folder containing mods/FS28_GlobalMarket.zip also resolves dynamically to FS28
    let custom_fs28 = temp_root.join("DedicatedServerProfile");
    fs::create_dir_all(custom_fs28.join("mods")).unwrap();
    fs::write(custom_fs28.join("mods").join("FS28_GlobalMarket.zip"), b"").unwrap();
    let p_custom28 = resolve_bridge_paths(&custom_fs28).expect("resolves custom FS28 profile");
    assert_eq!(p_custom28.game_edition, "FS28");
    assert!(p_custom28.bridge_root.ends_with("FS28_GlobalMarket"));

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn mock_backend_handles_moderation_transactions_and_great_demand() {
    let mock = LocalMockBackend::new();
    let state = mock
        .sync_whitelist(WhitelistSyncRequest {
            lang: "de".to_string(),
            items: vec![
                WhitelistItemDto {
                    name: "RYE".to_string(),
                    title: "Roggen".to_string(),
                    category: "GRAINS".to_string(),
                    item_type: "BULK".to_string(),
                    base_price: 0.34,
                    source: "FS25_CustomMap".to_string(),
                },
                WhitelistItemDto {
                    name: "POTATO".to_string(),
                    title: "Kartoffeln".to_string(),
                    category: "ROOT_VEG".to_string(),
                    item_type: "BULK".to_string(),
                    base_price: 0.22,
                    source: "BASEGAME".to_string(),
                },
            ],
        })
        .unwrap();

    assert_eq!(
        state
            .items
            .iter()
            .find(|i| i.fill_type == "RYE")
            .unwrap()
            .status,
        FillTypeStatus::Pending
    );
    assert_eq!(
        state
            .items
            .iter()
            .find(|i| i.fill_type == "POTATO")
            .unwrap()
            .status,
        FillTypeStatus::Approved
    );

    mock.trigger_great_demand(Some("WHEAT"));
    let after_sell = mock
        .submit_transaction(TransactionRequest {
            tx_id: "1729512015_1".to_string(),
            fill_type: "WHEAT".to_string(),
            amount: 200_000,
            tx_type: TransactionType::Sell,
        })
        .unwrap();

    let wheat = after_sell
        .items
        .iter()
        .find(|i| i.fill_type == "WHEAT")
        .unwrap();
    assert_eq!(wheat.fill_level, 1_550_000);
    assert!(!wheat.great_demand);
    assert_eq!(wheat.great_demand_remaining, 0);
}

#[test]
fn end_to_end_bridge_processes_whitelist_transaction_and_cleans_up_on_shutdown() {
    let test_dir = env::temp_dir().join(format!("fs25_gm_e2e_bridge_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&test_dir);

    let profile_dir = test_dir.join("FarmingSimulator2025");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(profile_dir.join("gameSettings.xml"), "<gameSettings/>").unwrap();

    let config_file = test_dir.join("config").join("settings.json");
    let init_cfg = AppConfig {
        access_tier: AccessTier::Developer,
        backend_mode: BackendMode::Mock,
        fs25_profile_path: Some(profile_dir.to_string_lossy().into_owned()),
        ..AppConfig::default()
    };
    init_cfg.save_to_file(&config_file).unwrap();

    let manager = BridgeManager::new(config_file, None);
    let bridge_root = profile_dir.join("modSettings").join("FS25_GlobalMarket");
    let write_dir = bridge_root.join("write");
    let read_dir = bridge_root.join("read");
    let online_file = bridge_root.join("globalMarketOnline.xml");

    for _ in 0..30 {
        if online_file.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(online_file.exists(), "globalMarketOnline.xml must be created");

    fs::write(
        write_dir.join("whitelist_1729512000.xml"),
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="updateWhitelist" lang="de">
    <whitelist>
        <item name="WHEAT" title="Weizen" category="GRAINS" type="BULK" basePrice="0.333000" source="BASEGAME" />
        <item name="SPELT" title="Dinkel" category="GRAINS" type="BULK" basePrice="0.360000" source="FS25_MapName" />
    </whitelist>
</gmDataSync>"#,
    )
    .unwrap();

    fs::write(
        write_dir.join("transaction_1729512015_1.xml"),
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="transaction">
    <entry fillType="WHEAT" amount="50000" type="BUY" />
</gmDataSync>"#,
    )
    .unwrap();

    for _ in 0..40 {
        let remaining_xmls: usize = fs::read_dir(&write_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "xml"))
            .count();
        if remaining_xmls == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let snap = manager.get_snapshot();
    assert_eq!(snap.state, BridgeConnectionState::Online);
    assert_eq!(snap.access_tier, AccessTier::Developer);
    assert!(
        snap.heartbeat_seq >= 1 && snap.last_heartbeat_unix.is_some(),
        "Online bridge worker must advance heartbeat_seq and record last_heartbeat_unix"
    );
    assert_eq!(snap.pending_count, 1, "SPELT from mod map must be PENDING");

    let logs = manager.get_logs();
    assert!(
        logs.iter()
            .any(|l| l.key.as_deref() == Some("logTransactionBooked")),
        "Activity log must emit structured i18n log keys"
    );

    let read_files: Vec<PathBuf> = fs::read_dir(&read_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    assert!(!read_files.is_empty(), "read/gmDataSync_*.xml must be written");

    let combined_xml: String = read_files
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect();
    assert!(combined_xml.contains("fillType=\"WHEAT\" fillLevel=\"1300000\""));
    assert!(combined_xml.contains("fillType=\"SPELT\""));
    assert!(combined_xml.contains("status=\"PENDING\""));

    manager.shutdown();
    assert!(
        !online_file.exists(),
        "globalMarketOnline.xml must be deleted immediately on shutdown"
    );
    let remaining_read_files = fs::read_dir(&read_dir).unwrap().flatten().count();
    assert_eq!(
        remaining_read_files, 0,
        "Unread read/gmDataSync_*.xml snapshots must be cleaned up on shutdown"
    );

    let _ = fs::remove_dir_all(&test_dir);
}

#[test]
fn diagnostic_report_redacts_usernames_and_market_tokens() {
    use globalmarket_client_lib::bridge::coordinator::{BridgeSnapshot, LogEntry, LogLevel};
    use globalmarket_client_lib::diagnostics::build_diagnostic_report;

    let user_profile = env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\TestFarmer".to_string());
    let snap = BridgeSnapshot {
        state: BridgeConnectionState::Online,
        game_edition: Some("FS25".to_string()),
        access_tier: AccessTier::Developer,
        backend_mode: BackendMode::Mock,
        backend_url: "http://127.0.0.1:8000".to_string(),
        api_version: Some("2.0.0".to_string()),
        market_name: "DSA Test Market".to_string(),
        market_logo_url: None,
        profile_path: Some(format!("{user_profile}\\Documents\\My Games\\FarmingSimulator2025")),
        bridge_path: None,
        exe_version: "1.0.0".to_string(),
        min_exe_version: "1.0.0".to_string(),
        approved_count: 10,
        pending_count: 2,
        rejected_count: 0,
        read_queue_count: 0,
        write_queue_count: 0,
        active_great_demands: vec!["Wheat".to_string()],
        active_great_demand_codes: vec!["WHEAT".to_string()],
        heartbeat_seq: 7,
        last_heartbeat_unix: Some(1_729_512_002),
        last_sync_unix: Some(1_729_512_000),
        last_error: None,
    };

    let cfg = AppConfig {
        client_id: "a3f901b842e7c10d".to_string(),
        access_tier: AccessTier::Developer,
        market_token: Some("SUPER_SECRET_PRIVATE_MARKET_KEY_999".to_string()),
        ..AppConfig::default()
    };

    let logs = vec![LogEntry {
        timestamp_ms: 1_729_512_005_123,
        level: LogLevel::Info,
        message: format!("Loaded profile path: {user_profile}\\Documents (cid=a3f901b842e7c10d)"),
        key: None,
        params: None,
    }];

    let report = build_diagnostic_report(&snap, &cfg, &logs);
    assert!(
        !report.contains("SUPER_SECRET_PRIVATE_MARKET_KEY_999"),
        "Must never leak private market token!"
    );
    assert!(
        !report.contains("a3f901b842e7c10d"),
        "Must never leak full 16-char client_id in public Discord diagnostic report!"
    );
    assert!(report.contains("client_slot_prefix: \"a3f901…\""));
    assert!(report.contains("lifebit_seq=7"));
    assert!(report.contains("configured (redacted)"));
    assert!(report.contains("game_edition: \"FS25\""));
    assert!(report.contains("tier=developer"));
    assert!(report.contains("approved=10, pending=2, rejected=0"));
    if env::var("USERPROFILE").is_ok() {
        assert!(
            !report.contains(&user_profile),
            "Must scrub USERPROFILE prefix from diagnostic report!"
        );
    }
}

#[test]
fn persistent_client_id_prefixes_transactions_without_collisions_and_remains_idempotent() {
    let dir = env::temp_dir().join(format!("fs25_gm_cid_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let cfg_path = dir.join("settings.json");

    let cfg_a = AppConfig::default();
    let cfg_b = AppConfig::default();
    assert_eq!(cfg_a.client_id.len(), 16);
    assert_ne!(
        cfg_a.client_id, cfg_b.client_id,
        "Two installations must generate distinct random client_ids"
    );

    cfg_a.save_to_file(&cfg_path).unwrap();
    let reloaded = AppConfig::load_from_file(&cfg_path);
    assert_eq!(reloaded.client_id, cfg_a.client_id);

    // Simulate upgrading an older settings.json that had no clientId or logAutoScroll field yet
    fs::write(&cfg_path, r#"{"language":"en","backendMode":"mock"}"#).unwrap();
    let upgraded = AppConfig::load_from_file(&cfg_path);
    assert_eq!(upgraded.client_id.len(), 16);
    assert_eq!(upgraded.language, "en");
    assert!(
        upgraded.log_auto_scroll,
        "Upgraded config must default log_auto_scroll to true"
    );

    let mut paused_scroll = upgraded.clone();
    paused_scroll.log_auto_scroll = false;
    paused_scroll.save_to_file(&cfg_path).unwrap();
    assert!(!AppConfig::load_from_file(&cfg_path).log_auto_scroll);

    // Two players trading in the same Unix second produce distinct tx_ids, while local retries stay identical
    let tx_player_a = qualify_transaction_id(&cfg_a.client_id, "1729512015_1");
    let tx_player_b = qualify_transaction_id(&cfg_b.client_id, "1729512015_1");
    assert_ne!(tx_player_a, tx_player_b);
    assert_eq!(
        qualify_transaction_id(&cfg_a.client_id, &tx_player_a),
        tx_player_a,
        "qualify_transaction_id must be idempotent"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fastapi_backend_honors_openapi_v2_etag_304_transactions_and_401() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback test server");
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let served_requests = Arc::new(AtomicUsize::new(0));
    let served_clone = Arc::clone(&served_requests);

    let server_thread = thread::spawn(move || {
        // Serve up to 8 sequential HTTP requests on 127.0.0.1 with a bounded deadline
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        for step in 0..8 {
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("accept client connection: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();

            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();

            let mut headers = Vec::new();
            let mut content_length = 0_usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    break;
                }
                if let Some((k, v)) = trimmed.split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                    headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                }
            }

            let mut body_bytes = vec![0_u8; content_length];
            if content_length > 0 {
                reader.read_exact(&mut body_bytes).unwrap();
            }
            let body_str = String::from_utf8_lossy(&body_bytes).into_owned();
            served_clone.fetch_add(1, Ordering::SeqCst);

            let get_header = |name: &str| -> Option<String> {
                headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            };

            let response = match step {
                // 0: GET /api/v1/meta/status
                0 => {
                    assert!(request_line.starts_with("GET /api/v1/meta/status "));
                    assert_eq!(
                        get_header("x-market-token").as_deref(),
                        Some("valid-community-token")
                    );
                    assert_eq!(
                        get_header("x-client-id").as_deref(),
                        Some("c84f92a17b0e3d41")
                    );
                    let json = r#"{"online":true,"min_exe_version":"1.0.0","poll_interval_sec":10,"market_name":"DSA Community Market","api_version":"2.0.0","market_logo_url":"/assets/community-logo.png"}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                // 1: POST /api/v1/market/whitelist -> returns ETag "etag-v1"
                1 => {
                    assert!(request_line.starts_with("POST /api/v1/market/whitelist "));
                    assert!(body_str.contains("\"lang\":\"de\""));
                    assert!(body_str.contains("\"type\":\"BULK\""));
                    assert!(body_str.contains("\"base_price\":0.333"));
                    let json = r#"{"updated_at":1729512000,"items":[{"fill_type":"WHEAT","fill_level":1400000,"trend":0.88,"great_demand":false,"great_demand_bonus":0.0,"great_demand_remaining":0,"status":"APPROVED"}]}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"etag-v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                // 2: GET /api/v1/market/state with If-None-Match: "etag-v1" -> 304 Not Modified
                2 => {
                    assert!(request_line.starts_with("GET /api/v1/market/state "));
                    assert_eq!(
                        get_header("if-none-match").as_deref(),
                        Some("\"etag-v1\"")
                    );
                    "HTTP/1.1 304 Not Modified\r\nETag: \"etag-v1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                }
                // 3: POST /api/v1/market/transactions -> updates stock & returns ETag "etag-v2"
                3 => {
                    assert!(request_line.starts_with("POST /api/v1/market/transactions "));
                    assert!(body_str.contains("\"tx_id\":\"c84f92a17b0e3d41_1729512015_1\""));
                    assert!(body_str.contains("\"type\":\"BUY\""));
                    assert!(body_str.contains("\"amount\":50000"));
                    let json = r#"{"updated_at":1729512015,"items":[{"fill_type":"WHEAT","fill_level":1350000,"trend":0.90,"great_demand":true,"great_demand_bonus":0.25,"great_demand_remaining":200000,"status":"APPROVED"}]}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"etag-v2\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                // 4: GET /api/v1/market/state with If-None-Match: "etag-v2" -> 304 Not Modified (must return updated 1,350,000 L!)
                4 => {
                    assert!(request_line.starts_with("GET /api/v1/market/state "));
                    assert_eq!(
                        get_header("if-none-match").as_deref(),
                        Some("\"etag-v2\"")
                    );
                    "HTTP/1.1 304 Not Modified\r\nETag: \"etag-v2\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                }
                // 5: GET /api/v1/market/state after token change -> ETag cache cleared, server rejects invalid token with 401
                5 => {
                    assert!(request_line.starts_with("GET /api/v1/market/state "));
                    assert_eq!(
                        get_header("if-none-match"),
                        None,
                        "Changing X-Market-Token must invalidate cached ETag"
                    );
                    assert_eq!(
                        get_header("x-market-token").as_deref(),
                        Some("wrong-token")
                    );
                    assert_eq!(
                        get_header("x-client-id").as_deref(),
                        Some("c84f92a17b0e3d41")
                    );
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                }
                // 6: GET /api/v1/meta/status -> 403 TOKEN_SLOT_LIMIT_REACHED (16 slots for standard Logo/Private key)
                6 => {
                    assert!(request_line.starts_with("GET /api/v1/meta/status "));
                    assert_eq!(
                        get_header("x-client-id").as_deref(),
                        Some("c84f92a17b0e3d41")
                    );
                    let json = r#"{"detail":{"code":"TOKEN_SLOT_LIMIT_REACHED","max_slots":16}}"#;
                    format!(
                        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                // 7: GET /api/v1/meta/status -> 403 TOKEN_SLOT_LIMIT_REACHED (32 slots for Combo key)
                7 => {
                    assert!(request_line.starts_with("GET /api/v1/meta/status "));
                    let json = r#"{"detail":{"code":"TOKEN_SLOT_LIMIT_REACHED","max_slots":32}}"#;
                    format!(
                        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                _ => unreachable!(),
            };

            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Write);
            drop(reader);
            let _ = stream.read(&mut [0_u8; 64]);
        }
    });

    let client = FastApiBackend::new(
        format!("http://{addr}/"),
        Some("valid-community-token".to_string()),
        "c84f92a17b0e3d41".to_string(),
    )
    .unwrap();

    // Step 0: Handshake
    let hs = client.check_status().unwrap();
    assert!(hs.online);
    assert_eq!(hs.min_exe_version, "1.0.0");
    assert_eq!(hs.market_name, "DSA Community Market");
    assert_eq!(hs.api_version, "2.0.0");
    assert_eq!(
        hs.market_logo_url,
        Some(format!("http://{addr}/assets/community-logo.png"))
    );

    // Step 1: Whitelist sync
    let wl_state = client
        .sync_whitelist(WhitelistSyncRequest {
            lang: "de".to_string(),
            items: vec![WhitelistItemDto {
                name: "WHEAT".to_string(),
                title: "Weizen".to_string(),
                category: "GRAINS".to_string(),
                item_type: "BULK".to_string(),
                base_price: 0.333,
                source: "BASEGAME".to_string(),
            }],
        })
        .unwrap();
    assert_eq!(wl_state.items[0].fill_level, 1_400_000);

    // Step 2: 304 Not Modified returns cached v1 state
    let cached_v1 = client.fetch_market_state().unwrap();
    assert_eq!(cached_v1.items[0].fill_level, 1_400_000);

    // Step 3: Transaction updates ETag cache to v2
    let tx_state = client
        .submit_transaction(TransactionRequest {
            tx_id: "c84f92a17b0e3d41_1729512015_1".to_string(),
            fill_type: "WHEAT".to_string(),
            amount: 50_000,
            tx_type: TransactionType::Buy,
        })
        .unwrap();
    assert_eq!(tx_state.items[0].fill_level, 1_350_000);
    assert!(tx_state.items[0].great_demand);

    // Step 4: Subsequent 304 Not Modified returns updated v2 state (never stale v1!)
    let cached_v2 = client.fetch_market_state().unwrap();
    assert_eq!(cached_v2.items[0].fill_level, 1_350_000);
    assert!(cached_v2.items[0].great_demand);

    // Step 5: Switching token clears ETag cache and surfaces 401 Unauthorized
    client.update_connection_config(
        format!("http://{addr}"),
        Some("wrong-token".to_string()),
        "c84f92a17b0e3d41".to_string(),
    );
    let auth_err = client.fetch_market_state().unwrap_err();
    assert!(matches!(auth_err, ApiError::Unauthorized));

    // Step 6 & 7: Key slot limits (16 for Logo/Private keys, 32 for Combo keys) map to SlotLimitReached
    let slot_err_16 = client.check_status().unwrap_err();
    assert!(matches!(
        slot_err_16,
        ApiError::SlotLimitReached { max_slots: 16 }
    ));

    let slot_err_32 = client.check_status().unwrap_err();
    assert!(matches!(
        slot_err_32,
        ApiError::SlotLimitReached { max_slots: 32 }
    ));

    server_thread.join().unwrap();
    assert_eq!(served_requests.load(Ordering::SeqCst), 8);
}

#[test]
fn access_tier_keys_and_five_language_crop_localization_work() {
    // Use ephemeral test-only digests so production access keys never appear in repository source or tests
    let dummy_ext = digest_access_key("UNIT-TEST-SUPPORT-ONLY");
    let dummy_dev = digest_access_key("UNIT-TEST-DEVELOPER-ONLY");

    assert_eq!(
        verify_access_key_with_digests("UNIT-TEST-SUPPORT-ONLY", dummy_ext, dummy_dev),
        Some(AccessTier::Extended)
    );
    assert_eq!(
        verify_access_key_with_digests("  UNIT-TEST-SUPPORT-ONLY  ", dummy_ext, dummy_dev),
        Some(AccessTier::Extended)
    );
    assert_eq!(
        verify_access_key_with_digests("UNIT-TEST-DEVELOPER-ONLY", dummy_ext, dummy_dev),
        Some(AccessTier::Developer)
    );
    assert_eq!(
        verify_access_key_with_digests("unit-test-developer-only", dummy_ext, dummy_dev),
        None
    );
    assert_eq!(verify_access_key("UNIT-TEST-DEVELOPER-ONLY"), None);
    assert_eq!(verify_access_key("wrong-key"), None);
    assert_eq!(verify_access_key(""), None);

    // Extended mode is strictly ephemeral: resets to User on disk/restart AND resets custom URL / Mock mode
    let dir = env::temp_dir().join(format!("fs25_gm_tier_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let cfg_path = dir.join("settings.json");

    let tampered = AppConfig {
        access_tier: AccessTier::Extended,
        backend_mode: BackendMode::Mock,
        backend_url: "http://evil.invalid:9999".to_string(),
        ..AppConfig::default()
    };
    tampered.save_to_file(&cfg_path).unwrap();
    let sanitized = AppConfig::load_from_file(&cfg_path);
    assert_eq!(
        sanitized.access_tier,
        AccessTier::User,
        "Extended diagnostic mode must always reset to User on restart"
    );
    assert_eq!(sanitized.backend_mode, BackendMode::Live);
    assert_eq!(sanitized.backend_url, DEFAULT_OFFICIAL_BACKEND_URL);

    // Even if someone manually edits settings.json to `"accessTier":"extended"`, startup resets it to User
    fs::write(
        &cfg_path,
        r#"{"accessTier":"extended","backendMode":"mock","backendUrl":"http://evil.invalid:9999"}"#,
    )
    .unwrap();
    let from_raw = AppConfig::load_from_file(&cfg_path);
    assert_eq!(from_raw.access_tier, AccessTier::User);
    assert_eq!(from_raw.backend_mode, BackendMode::Live);

    // Developer tier persists across restarts along with Mock mode and custom server URL
    let dev_cfg = AppConfig {
        access_tier: AccessTier::Developer,
        backend_mode: BackendMode::Mock,
        backend_url: "http://127.0.0.1:8088".to_string(),
        ..AppConfig::default()
    };
    dev_cfg.save_to_file(&cfg_path).unwrap();
    let loaded_dev = AppConfig::load_from_file(&cfg_path);
    assert_eq!(loaded_dev.access_tier, AccessTier::Developer);
    assert_eq!(loaded_dev.backend_mode, BackendMode::Mock);
    assert_eq!(loaded_dev.backend_url, "http://127.0.0.1:8088");

    // Extended diagnostic mode also auto-expires after its TTL while running
    let ttl_cfg_path = dir.join("ttl_settings.json");
    let ttl_init_cfg = AppConfig {
        fs25_profile_path: Some(dir.join("unconfigured_profile").to_string_lossy().into_owned()),
        ..AppConfig::default()
    };
    ttl_init_cfg.save_to_file(&ttl_cfg_path).unwrap();
    let mgr = BridgeManager::new(ttl_cfg_path, None);
    let unlocked = mgr
        .unlock_access_tier_with_digests(
            "UNIT-TEST-SUPPORT-ONLY",
            dummy_ext,
            dummy_dev,
            Duration::from_millis(80),
        )
        .unwrap();
    assert_eq!(unlocked.access_tier, AccessTier::Extended);
    assert_eq!(mgr.get_snapshot().access_tier, AccessTier::Extended);

    thread::sleep(Duration::from_millis(120));
    assert_eq!(
        mgr.get_snapshot().access_tier,
        AccessTier::User,
        "Extended mode must auto-expire to User once TTL elapses"
    );
    assert!(mgr
        .get_logs()
        .iter()
        .any(|l| l.key.as_deref() == Some("logExtendedExpired")));
    mgr.shutdown();

    let _ = fs::remove_dir_all(&dir);

    // 5-language basegame fillType translation
    assert_eq!(localize_fill_type("WHEAT", "de"), Some("Weizen"));
    assert_eq!(localize_fill_type("WHEAT", "en"), Some("Wheat"));
    assert_eq!(localize_fill_type("WHEAT", "fr"), Some("Blé"));
    assert_eq!(localize_fill_type("WHEAT", "es"), Some("Trigo"));
    assert_eq!(localize_fill_type("WHEAT", "pl"), Some("Pszenica"));
    assert_eq!(localize_fill_type("CUSTOM_MOD_CROP", "en"), None);
}

#[test]
fn mock_community_logo_preview_and_rfc4648_base64_work() {
    // RFC 4648 §10 canonical Base64 test vectors
    assert_eq!(encode_base64(b""), "");
    assert_eq!(encode_base64(b"f"), "Zg==");
    assert_eq!(encode_base64(b"fo"), "Zm8=");
    assert_eq!(encode_base64(b"foo"), "Zm9v");
    assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
    assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
    assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");

    let dir = env::temp_dir().join(format!("fs25_gm_mock_logo_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let profile_dir = dir.join("FarmingSimulator2025");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(profile_dir.join("gameSettings.xml"), "<gameSettings/>").unwrap();

    // Minimal valid 1x1 transparent PNG
    let tiny_png: [u8; 67] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let logo_path = dir.join("sample_logo.png");
    fs::write(&logo_path, tiny_png).unwrap();

    let cfg_file = dir.join("settings.json");
    let init_cfg = AppConfig {
        fs25_profile_path: Some(dir.join("unconfigured_profile").to_string_lossy().into_owned()),
        ..AppConfig::default()
    };
    init_cfg.save_to_file(&cfg_file).unwrap();

    let mgr = BridgeManager::new(cfg_file, None);
    // Standard user cannot set mock logos
    assert!(mgr
        .mock_set_community_logo(Some(logo_path.to_string_lossy().into_owned()))
        .is_err());

    // Unlock Developer mode via test-only digest + switch to Mock backend
    let dummy_ext = digest_access_key("UNIT-TEST-SUPPORT-ONLY");
    let dummy_dev = digest_access_key("UNIT-TEST-DEVELOPER-ONLY");
    mgr.unlock_access_tier_with_digests(
        "UNIT-TEST-DEVELOPER-ONLY",
        dummy_ext,
        dummy_dev,
        Duration::from_secs(1800),
    )
    .unwrap();
    let mut dev_cfg = mgr.get_config();
    dev_cfg.backend_mode = BackendMode::Mock;
    dev_cfg.fs25_profile_path = Some(profile_dir.to_string_lossy().into_owned());
    mgr.update_config(dev_cfg).unwrap();

    let snap_with_logo = mgr
        .mock_set_community_logo(Some(logo_path.to_string_lossy().into_owned()))
        .unwrap();
    assert!(snap_with_logo
        .market_logo_url
        .as_deref()
        .is_some_and(|u| u.starts_with("data:image/png;base64,")));

    let snap_reset = mgr.mock_set_community_logo(None).unwrap();
    assert_eq!(snap_reset.market_logo_url, None);

    // Non-blocking GUI quit helper removes globalMarketOnline.xml immediately and exits worker cleanly
    let online_xml = profile_dir
        .join("modSettings")
        .join("FS25_GlobalMarket")
        .join("globalMarketOnline.xml");
    let worker_handle = mgr.prepare_fast_exit();
    assert!(
        !online_xml.exists(),
        "prepare_fast_exit must remove globalMarketOnline.xml immediately"
    );
    if let Some(h) = worker_handle {
        h.join().unwrap();
    }

    mgr.shutdown();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn official_domains_and_trusted_logo_cdn_validation_work() {
    assert_eq!(DEFAULT_OFFICIAL_BACKEND_URL, "https://gm.bydora.de");
    assert_eq!(OFFICIAL_DEV_BACKEND_URL, "https://dev.gm.bydora.de");
    assert_eq!(OFFICIAL_LOGO_CDN_URL, "https://img.gm.bydora.de");

    // Official Logo CDN URL with active token is accepted
    assert_eq!(
        resolve_safe_logo_url(
            Some("https://img.gm.bydora.de/logos/breide.png".to_string()),
            DEFAULT_OFFICIAL_BACKEND_URL,
            true
        ),
        Some("https://img.gm.bydora.de/logos/breide.png".to_string())
    );

    // Bare relative asset key resolves directly against https://img.gm.bydora.de
    assert_eq!(
        resolve_safe_logo_url(
            Some("logos/breide.webp".to_string()),
            DEFAULT_OFFICIAL_BACKEND_URL,
            true
        ),
        Some("https://img.gm.bydora.de/logos/breide.webp".to_string())
    );

    // Root-relative path resolves against active official Prod/Dev server
    assert_eq!(
        resolve_safe_logo_url(
            Some("/static/logo.png".to_string()),
            OFFICIAL_DEV_BACKEND_URL,
            true
        ),
        Some("https://dev.gm.bydora.de/static/logo.png".to_string())
    );

    // Without an active Community/Logo token, custom logos are ignored
    assert_eq!(
        resolve_safe_logo_url(
            Some("https://img.gm.bydora.de/logos/breide.png".to_string()),
            DEFAULT_OFFICIAL_BACKEND_URL,
            false
        ),
        None
    );

    // Untrusted third-party tracking domains, subdomain spoofing, userinfo tricks, and cleartext HTTP are blocked
    for rejected in [
        "https://tracker.example.org/spy.png",
        "https://img.gm.bydora.de.evil.com/spy.png",
        "https://img.gm.bydora.de@evil.com/spy.png",
        "http://img.gm.bydora.de/logos/breide.png",
    ] {
        assert_eq!(
            resolve_safe_logo_url(
                Some(rejected.to_string()),
                DEFAULT_OFFICIAL_BACKEND_URL,
                true
            ),
            None,
            "Untrusted logo URL must be rejected: {rejected}"
        );
    }
}
