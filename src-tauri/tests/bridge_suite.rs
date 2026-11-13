//! Comprehensive integration & unit test suite for the FS25 GlobalMarket Rust Core (Phases 2–4 & 3.5).

use globalmarket_client_lib::api::fastapi::{
    derive_handshake_branding, resolve_safe_logo_url, FastApiBackend,
};
use globalmarket_client_lib::api::mock::LocalMockBackend;
use globalmarket_client_lib::api::types::{
    FillTypeStatus, MarketItemDto, TransactionRequest, TransactionType, WhitelistItemDto,
    WhitelistSyncRequest,
};
use globalmarket_client_lib::api::{ApiError, BackendService};
use globalmarket_client_lib::bridge::coordinator::{
    encode_base64, BridgeConnectionState, BridgeManager,
};
use globalmarket_client_lib::bridge::dds::{
    convert_logo_bytes_to_dds_bytes, decode_dds_to_rgba8, is_valid_cached_logo_dds,
    logo_cache_hash12, logo_rel_path_for_hash, EXPECTED_LOGO_DDS_BYTES, LOGO_CANVAS_HEIGHT,
    LOGO_CANVAS_WIDTH, LOGO_MIPMAP_COUNT,
};
use globalmarket_client_lib::bridge::l10n::localize_fill_type;
use globalmarket_client_lib::bridge::paths::resolve_bridge_paths;
use globalmarket_client_lib::bridge::xml::{
    is_version_sufficient, parse_incoming_write_xml, remove_heartbeat_xml, sanitize_logo_rel_path,
    sanitize_market_name, write_heartbeat_xml, write_market_data_xml, HeartbeatBranding,
    IncomingPacket, MAX_MARKET_NAME_BYTES, MAX_RETAINED_MARKET_SYNC_FILES,
};
use globalmarket_client_lib::config::vault::{
    compute_token_profile_id, seal_vault_bytes, unseal_vault_bytes, TokenVault,
};
use globalmarket_client_lib::config::{
    digest_access_key, qualify_transaction_id, verify_access_key, verify_access_key_with_digests,
    AccessTier, AppConfig, BackendMode, DEFAULT_OFFICIAL_BACKEND_URL, OFFICIAL_DEV_BACKEND_URL,
    OFFICIAL_LOGO_CDN_URL,
};
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn writes_heartbeat_and_market_xml_strictly_without_utf8_bom() {
    let dir = env::temp_dir().join(format!("fs25_gm_xml_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let staging = dir.join(".staging");
    let read_dir = dir.join("read");
    let online_file = dir.join("globalMarketOnline.xml");

    write_heartbeat_xml(&staging, &online_file, true, "1.0.0.0", "1.0.0.0", None).unwrap();
    let online_bytes = fs::read(&online_file).unwrap();
    assert!(
        !online_bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
        "Must never emit UTF-8 BOM!"
    );
    let online_str = String::from_utf8(online_bytes).unwrap();
    assert!(online_str.contains("isConnected=\"true\""));
    assert!(online_str.contains("brandingTier=\"0\""));
    assert!(online_str.contains("isPrivateMarket=\"false\""));
    assert!(online_str.contains("hasCustomLogo=\"false\""));
    assert!(!online_str.contains("marketName="));
    assert!(!online_str.contains("logoFile="));
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
        branding_tier: 3,
        is_private_market: true,
        market_name: "DSA Test Market".to_string(),
        market_logo_url: None,
        market_logo_file: Some("logos/logo_a1b2c3d4e5f6.dds".to_string()),
        active_token_id: None,
        saved_tokens: Vec::new(),
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

#[test]
fn heartbeat_xml_3_tier_branding_and_lua_sanitization_rules_work() {
    let dir = env::temp_dir().join(format!("fs25_gm_branding_xml_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let staging = dir.join(".staging");
    let online_file = dir.join("globalMarketOnline.xml");

    // Tier 1: Official Global Market + custom logo (must NOT emit marketName)
    let tier1 = HeartbeatBranding {
        branding_tier: 1,
        is_private_market: false,
        market_name: Some("Ignored Name On Tier 1".to_string()),
        has_custom_logo: true,
        logo_file: Some("logos/logo_a1b2c3d4e5f6.dds".to_string()),
    };
    write_heartbeat_xml(&staging, &online_file, true, "1.0.0", "1.0.0", Some(&tier1)).unwrap();
    let xml_t1 = fs::read_to_string(&online_file).unwrap();
    assert!(xml_t1.contains("brandingTier=\"1\""));
    assert!(xml_t1.contains("isPrivateMarket=\"false\""));
    assert!(!xml_t1.contains("marketName="));
    assert!(xml_t1.contains("hasCustomLogo=\"true\""));
    assert!(xml_t1.contains("logoFile=\"logos/logo_a1b2c3d4e5f6.dds\""));

    // Tier 2: Private Market + standard logo (must NOT emit logoFile; escapes XML characters)
    let tier2 = HeartbeatBranding {
        branding_tier: 2,
        is_private_market: true,
        market_name: Some("  D&S \"Agrar\" <Ost>  ".to_string()),
        has_custom_logo: false,
        logo_file: Some("logos/logo_ignored.dds".to_string()),
    };
    write_heartbeat_xml(&staging, &online_file, true, "1.0.0", "1.0.0", Some(&tier2)).unwrap();
    let xml_t2 = fs::read_to_string(&online_file).unwrap();
    assert!(xml_t2.contains("brandingTier=\"2\""));
    assert!(xml_t2.contains("isPrivateMarket=\"true\""));
    assert!(xml_t2.contains("marketName=\"D&amp;S &quot;Agrar&quot; &lt;Ost&gt;\""));
    assert!(xml_t2.contains("hasCustomLogo=\"false\""));
    assert!(!xml_t2.contains("logoFile="));

    // Tier 3: Private Market + custom logo
    let tier3 = HeartbeatBranding {
        branding_tier: 3,
        is_private_market: true,
        market_name: Some("LSMT Community Markt".to_string()),
        has_custom_logo: true,
        logo_file: Some("logos/logo_a1b2c3d4e5f6.dds".to_string()),
    };
    write_heartbeat_xml(&staging, &online_file, true, "1.0.0", "1.0.0", Some(&tier3)).unwrap();
    let xml_t3 = fs::read_to_string(&online_file).unwrap();
    assert!(xml_t3.contains("brandingTier=\"3\""));
    assert!(xml_t3.contains("isPrivateMarket=\"true\""));
    assert!(xml_t3.contains("marketName=\"LSMT Community Markt\""));
    assert!(xml_t3.contains("hasCustomLogo=\"true\""));
    assert!(xml_t3.contains("logoFile=\"logos/logo_a1b2c3d4e5f6.dds\""));

    // Disconnected heartbeat always resets branding attributes to safe Tier 0 defaults
    write_heartbeat_xml(
        &staging,
        &online_file,
        false,
        "1.0.0",
        "1.0.0",
        Some(&tier3),
    )
    .unwrap();
    let xml_offline = fs::read_to_string(&online_file).unwrap();
    assert!(xml_offline.contains("isConnected=\"false\""));
    assert!(xml_offline.contains("brandingTier=\"0\""));
    assert!(xml_offline.contains("isPrivateMarket=\"false\""));
    assert!(xml_offline.contains("hasCustomLogo=\"false\""));
    assert!(!xml_offline.contains("marketName="));
    assert!(!xml_offline.contains("logoFile="));

    // UTF-8 multi-byte truncation strictly respects 48-byte Lua limit without splitting German umlauts
    let long_umlaut_name = "Süddeutsche Agrargemeinschaft für Öko-Landbau ÄÖÜ 2026";
    let clean_umlaut = sanitize_market_name(long_umlaut_name).unwrap();
    assert!(clean_umlaut.len() <= MAX_MARKET_NAME_BYTES);
    assert!(std::str::from_utf8(clean_umlaut.as_bytes()).is_ok());

    // Relative logoFile validation matches GlobalMarketBranding.sanitizeLogoRelPath
    assert_eq!(
        sanitize_logo_rel_path("logos/logo_a1b2c3d4e5f6.dds"),
        Some("logos/logo_a1b2c3d4e5f6.dds".to_string())
    );
    assert_eq!(
        sanitize_logo_rel_path("logos\\logo_a1b2c3d4e5f6.DDS"),
        Some("logos/logo_a1b2c3d4e5f6.DDS".to_string())
    );
    assert_eq!(
        sanitize_logo_rel_path("custom_banner.dds"),
        Some("custom_banner.dds".to_string())
    );
    for invalid_logo in [
        "",
        ".dds",
        "logos/.dds",
        "../secret.dds",
        "logos/../secret.dds",
        "C:/temp/logo.dds",
        "logos/sub/nested.dds",
        "logos/logo_a1b2.png",
        "logos/logo_a1b2.webp",
        "logos/logo space.dds",
    ] {
        assert_eq!(
            sanitize_logo_rel_path(invalid_logo),
            None,
            "Invalid logoFile path must be rejected: {invalid_logo}"
        );
    }

    // Hybrid handshake tier derivation (auto-derived vs explicit fields)
    assert_eq!(
        derive_handshake_branding(
            false,
            "LSMT",
            Some("https://img.gm.bydora.de/a.webp".to_string()),
            Some(3),
            Some(true),
            Some(true)
        ),
        (0, false, "Global Market".to_string(), None)
    );
    assert_eq!(
        derive_handshake_branding(
            true,
            "Global Market",
            Some("https://img.gm.bydora.de/a.webp".to_string()),
            None,
            None,
            None
        ),
        (
            1,
            false,
            "Global Market".to_string(),
            Some("https://img.gm.bydora.de/a.webp".to_string())
        )
    );
    assert_eq!(
        derive_handshake_branding(true, "LSMT Markt", None, None, None, None),
        (2, true, "LSMT Markt".to_string(), None)
    );
    assert_eq!(
        derive_handshake_branding(
            true,
            "LSMT Markt",
            Some("https://img.gm.bydora.de/lsmt.webp".to_string()),
            None,
            None,
            None
        ),
        (
            3,
            true,
            "LSMT Markt".to_string(),
            Some("https://img.gm.bydora.de/lsmt.webp".to_string())
        )
    );

    let _ = fs::remove_dir_all(&dir);
}

fn encode_test_webp(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let img = RgbaImage::from_pixel(width, height, Rgba(color));
    let mut buf = Vec::new();
    WebPEncoder::new_lossless(&mut buf)
        .write_image(img.as_raw(), width, height, ExtendedColorType::Rgba8)
        .expect("lossless WebP encode");
    buf
}

#[test]
fn pure_rust_webp_to_bottom_left_dxt5_dds_with_10_mipmaps_works() {
    // Case A: Wide banner 600x256 WebP -> scales proportionally to 512x218, placed bottom-left (x = 0..511, y = 38..255)
    let wide_webp = encode_test_webp(600, 256, [240, 160, 20, 255]);
    let wide_dds = convert_logo_bytes_to_dds_bytes(&wide_webp).unwrap();
    assert_eq!(wide_dds.len(), EXPECTED_LOGO_DDS_BYTES);
    assert_eq!(&wide_dds[0..4], b"DDS ");
    assert_eq!(
        u32::from_le_bytes([wide_dds[4], wide_dds[5], wide_dds[6], wide_dds[7]]),
        124
    );
    assert_eq!(
        u32::from_le_bytes([wide_dds[8], wide_dds[9], wide_dds[10], wide_dds[11]]),
        0x000A_1007
    );
    assert_eq!(
        u32::from_le_bytes([wide_dds[12], wide_dds[13], wide_dds[14], wide_dds[15]]),
        LOGO_CANVAS_HEIGHT
    );
    assert_eq!(
        u32::from_le_bytes([wide_dds[16], wide_dds[17], wide_dds[18], wide_dds[19]]),
        LOGO_CANVAS_WIDTH
    );
    assert_eq!(
        u32::from_le_bytes([wide_dds[28], wide_dds[29], wide_dds[30], wide_dds[31]]),
        LOGO_MIPMAP_COUNT
    );
    assert_eq!(&wide_dds[84..88], b"DXT5");
    assert_eq!(
        u32::from_le_bytes([wide_dds[108], wide_dds[109], wide_dds[110], wide_dds[111]]),
        0x0040_1008
    );

    let wide_decoded = decode_dds_to_rgba8(&wide_dds).unwrap();
    assert_eq!(
        (wide_decoded.width(), wide_decoded.height()),
        (LOGO_CANVAS_WIDTH, LOGO_CANVAS_HEIGHT)
    );
    // Top transparent margin (y = 10 < 38) must be transparent
    assert_eq!(wide_decoded.get_pixel(10, 10)[3], 0);
    assert_eq!(wide_decoded.get_pixel(500, 10)[3], 0);
    // Bottom-left and bottom-right must be opaque warm gold
    let bl = wide_decoded.get_pixel(10, 255);
    let br = wide_decoded.get_pixel(500, 255);
    assert!(bl[3] >= 250 && bl[0] > 200 && bl[1] > 120 && bl[2] < 60);
    assert!(br[3] >= 250 && br[0] > 200 && br[1] > 120 && br[2] < 60);

    // Case B: Square crest 512x512 WebP -> scales proportionally to 256x256, placed bottom-left (x = 0..255, y = 0..255)
    let square_webp = encode_test_webp(512, 512, [30, 200, 80, 255]);
    let square_dds = convert_logo_bytes_to_dds_bytes(&square_webp).unwrap();
    assert_eq!(square_dds.len(), EXPECTED_LOGO_DDS_BYTES);

    let square_decoded = decode_dds_to_rgba8(&square_dds).unwrap();
    let crest_left = square_decoded.get_pixel(128, 255);
    let crest_top_left = square_decoded.get_pixel(128, 10);
    assert!(crest_left[3] >= 250 && crest_left[1] > 170);
    assert!(crest_top_left[3] >= 250 && crest_top_left[1] > 170);
    // Right half (x >= 260) must remain completely transparent
    assert_eq!(square_decoded.get_pixel(400, 128)[3], 0);
    assert_eq!(square_decoded.get_pixel(511, 255)[3], 0);
}

#[test]
fn multi_token_logo_dds_cache_and_live_xml_switch_work_end_to_end() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener
        .set_nonblocking(true)
        .expect("nonblocking loopback listener");
    let addr = listener.local_addr().unwrap();

    let tier3_webp = Arc::new(encode_test_webp(600, 256, [220, 120, 30, 255]));
    let tier1_webp = Arc::new(encode_test_webp(512, 512, [40, 160, 230, 255]));
    let tier3_logo_hits = Arc::new(AtomicUsize::new(0));
    let tier1_logo_hits = Arc::new(AtomicUsize::new(0));
    let stop_server = Arc::new(AtomicBool::new(false));

    let tier3_webp_srv = Arc::clone(&tier3_webp);
    let tier1_webp_srv = Arc::clone(&tier1_webp);
    let tier3_hits_srv = Arc::clone(&tier3_logo_hits);
    let tier1_hits_srv = Arc::clone(&tier1_logo_hits);
    let stop_srv = Arc::clone(&stop_server);

    let server_thread = thread::spawn(move || {
        while !stop_srv.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut reader = BufReader::new(&mut stream);
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
                        continue;
                    }
                    let mut token_hdr: Option<String> = None;
                    let mut content_length: usize = 0;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty()
                        {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(':') {
                            let kl = k.trim().to_ascii_lowercase();
                            let vt = v.trim().to_string();
                            if kl == "x-market-token" {
                                token_hdr = Some(vt);
                            } else if kl == "content-length" {
                                content_length = vt.parse().unwrap_or(0);
                            }
                        }
                    }
                    if content_length > 0 {
                        let mut body = vec![0_u8; content_length];
                        let _ = reader.read_exact(&mut body);
                    }
                    drop(reader);

                    if request_line.starts_with("GET /api/v1/meta/status ") {
                        let body = match token_hdr.as_deref() {
                            Some("token-alpha-tier3") => format!(
                                "{{\"status\":\"online\",\"version\":\"2.1.0\",\"min_exe_version\":\"1.0.0\",\"branding_tier\":3,\"is_private_market\":true,\"market_name\":\"LSMT Community Markt\",\"has_custom_logo\":true,\"market_logo_url\":\"http://{addr}/logos/lsmt.webp\"}}"
                            ),
                            Some("token-bravo-tier1") => format!(
                                "{{\"status\":\"online\",\"version\":\"2.1.0\",\"min_exe_version\":\"1.0.0\",\"branding_tier\":1,\"is_private_market\":false,\"market_name\":\"Global Market\",\"has_custom_logo\":true,\"market_logo_url\":\"http://{addr}/logos/tier1.webp\"}}"
                            ),
                            _ => "{\"status\":\"online\",\"version\":\"2.1.0\",\"min_exe_version\":\"1.0.0\",\"branding_tier\":0,\"is_private_market\":false,\"market_name\":\"Global Market\",\"has_custom_logo\":false}".to_string(),
                        };
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if request_line.starts_with("GET /api/v1/market/state ") {
                        let body = r#"{"updated_at":1763058000,"items":[{"fill_type":"WHEAT","fill_level":1000000,"trend":1.0,"great_demand":false,"great_demand_bonus":0.0,"great_demand_remaining":0,"status":"APPROVED"}]}"#;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"e1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if request_line.starts_with("GET /logos/lsmt.webp ") {
                        tier3_hits_srv.fetch_add(1, Ordering::SeqCst);
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: image/webp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            tier3_webp_srv.len()
                        );
                        let _ = stream.write_all(hdr.as_bytes());
                        let _ = stream.write_all(&tier3_webp_srv);
                    } else if request_line.starts_with("GET /logos/tier1.webp ") {
                        tier1_hits_srv.fetch_add(1, Ordering::SeqCst);
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: image/webp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            tier1_webp_srv.len()
                        );
                        let _ = stream.write_all(hdr.as_bytes());
                        let _ = stream.write_all(&tier1_webp_srv);
                    } else {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    let _ = stream.flush();
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(15));
                }
                Err(_) => break,
            }
        }
    });

    let dir = env::temp_dir().join(format!(
        "fs25_gm_e2e_branding_cache_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    let profile_dir = dir.join("FarmingSimulator2025");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(profile_dir.join("gameSettings.xml"), "<gameSettings/>").unwrap();

    let cfg_file = dir.join("settings.json");
    let init_cfg = AppConfig {
        fs25_profile_path: Some(dir.join("unconfigured_profile").to_string_lossy().into_owned()),
        ..AppConfig::default()
    };
    init_cfg.save_to_file(&cfg_file).unwrap();

    let mgr = BridgeManager::new(cfg_file, None);
    let dummy_ext = digest_access_key("UNIT-TEST-SUPPORT-ONLY");
    let dummy_dev = digest_access_key("UNIT-TEST-DEVELOPER-ONLY");
    mgr.unlock_access_tier_with_digests(
        "UNIT-TEST-DEVELOPER-ONLY",
        dummy_ext,
        dummy_dev,
        Duration::from_secs(1800),
    )
    .unwrap();

    let url_a = format!("http://{addr}/logos/lsmt.webp");
    let url_b = format!("http://{addr}/logos/tier1.webp");
    let expected_rel_a = logo_rel_path_for_hash(&logo_cache_hash12(url_a.as_bytes()));
    let expected_rel_b = logo_rel_path_for_hash(&logo_cache_hash12(url_b.as_bytes()));

    let bridge_root = profile_dir.join("modSettings").join("FS25_GlobalMarket");
    let online_xml_path = bridge_root.join("globalMarketOnline.xml");

    let wait_for = |pred: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            thread::sleep(Duration::from_millis(40));
        }
        false
    };

    // 1. Configure Live endpoint and add/activate Token A (Tier 3: Private Market + WebP Logo)
    let mut cfg = mgr.get_config();
    cfg.backend_mode = BackendMode::Live;
    cfg.backend_url = format!("http://{addr}");
    cfg.fs25_profile_path = Some(profile_dir.to_string_lossy().into_owned());
    mgr.update_config(cfg).unwrap();

    let id_a = compute_token_profile_id("token-alpha-tier3");
    let id_b = compute_token_profile_id("token-bravo-tier1");
    mgr.add_market_token("token-alpha-tier3", Some("Stammtisch"))
        .unwrap();

    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && s.heartbeat_seq >= 1
                && s.branding_tier == 3
                && s.active_token_id.as_deref() == Some(id_a.as_str())
                && s.market_logo_file.as_deref() == Some(expected_rel_a.as_str())
        }),
        "Worker should sync Tier 3 branding and cache DDS for Token A"
    );

    let dds_path_a = bridge_root.join(&expected_rel_a);
    assert!(is_valid_cached_logo_dds(&dds_path_a));
    assert_eq!(tier3_logo_hits.load(Ordering::SeqCst), 1);

    // Verify learned vault metadata for Token A
    let snap_after_a = mgr.get_snapshot();
    let prof_a = snap_after_a
        .saved_tokens
        .iter()
        .find(|p| p.id == id_a)
        .expect("SavedTokenProfile for Token A must exist");
    assert_eq!(prof_a.custom_label.as_deref(), Some("Stammtisch"));
    assert_eq!(
        prof_a.detected_market_name.as_deref(),
        Some("LSMT Community Markt")
    );
    assert_eq!(prof_a.branding_tier, Some(3));
    assert!(prof_a.is_private_market);
    assert_eq!(
        prof_a.cached_logo_file.as_deref(),
        Some(expected_rel_a.as_str())
    );

    let xml_a = fs::read_to_string(&online_xml_path).unwrap();
    assert!(xml_a.contains("brandingTier=\"3\""));
    assert!(xml_a.contains("isPrivateMarket=\"true\""));
    assert!(xml_a.contains("marketName=\"LSMT Community Markt\""));
    assert!(xml_a.contains("hasCustomLogo=\"true\""));
    assert!(xml_a.contains(&format!("logoFile=\"{expected_rel_a}\"")));

    // 2. Add & switch to Token B (Tier 1: Official Market + Custom WebP Logo, NO marketName)
    mgr.add_market_token("token-bravo-tier1", Some("Unser Wappen"))
        .unwrap();

    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && s.heartbeat_seq >= 1
                && s.branding_tier == 1
                && s.active_token_id.as_deref() == Some(id_b.as_str())
                && s.market_logo_file.as_deref() == Some(expected_rel_b.as_str())
        }),
        "Worker should switch immediately to Tier 1 branding for Token B"
    );

    let dds_path_b = bridge_root.join(&expected_rel_b);
    assert!(is_valid_cached_logo_dds(&dds_path_b));
    assert_eq!(tier1_logo_hits.load(Ordering::SeqCst), 1);

    let snap_after_b = mgr.get_snapshot();
    assert_eq!(snap_after_b.saved_tokens.len(), 2);
    let prof_b = snap_after_b
        .saved_tokens
        .iter()
        .find(|p| p.id == id_b)
        .expect("SavedTokenProfile for Token B must exist");
    assert_eq!(prof_b.branding_tier, Some(1));
    assert!(!prof_b.is_private_market);

    let xml_b = fs::read_to_string(&online_xml_path).unwrap();
    assert!(xml_b.contains("brandingTier=\"1\""));
    assert!(xml_b.contains("isPrivateMarket=\"false\""));
    assert!(!xml_b.contains("marketName="));
    assert!(xml_b.contains("hasCustomLogo=\"true\""));
    assert!(xml_b.contains(&format!("logoFile=\"{expected_rel_b}\"")));

    // 3. Switch back to Token A via 1-click profile ID -> MUST reuse cached logos/logo_<hashA>.dds WITHOUT re-downloading!
    mgr.select_active_market_token(Some(id_a.clone())).unwrap();

    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && s.heartbeat_seq >= 1
                && s.branding_tier == 3
                && s.active_token_id.as_deref() == Some(id_a.as_str())
                && s.market_logo_file.as_deref() == Some(expected_rel_a.as_str())
                && fs::read_to_string(&online_xml_path)
                    .map(|x| x.contains(&format!("logoFile=\"{expected_rel_a}\"")))
                    .unwrap_or(false)
        }),
        "Worker should restore Token A logoFile from local DDS cache"
    );
    assert_eq!(
        tier3_logo_hits.load(Ordering::SeqCst),
        1,
        "Cached logos/logo_<hash>.dds must prevent redundant WebP downloads & re-encodes"
    );

    // 4. Switch to Official Market (None / Tier 0) while keeping both tokens saved in vault!
    mgr.select_active_market_token(None).unwrap();

    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && s.branding_tier == 0
                && s.active_token_id.is_none()
                && s.saved_tokens.len() == 2
                && !s.is_private_market
                && s.market_logo_file.is_none()
                && fs::read_to_string(&online_xml_path)
                    .map(|x| x.contains("brandingTier=\"0\"") && !x.contains("logoFile="))
                    .unwrap_or(false)
        }),
        "Switching to Official Market should immediately reset globalMarketOnline.xml to Tier 0 while retaining saved profiles"
    );

    // 5. Also verify Developer Mock mode Tier 0..3 simulation + DDS output
    let mut cfg_mock = mgr.get_config();
    cfg_mock.backend_mode = BackendMode::Mock;
    mgr.update_config(cfg_mock).unwrap();
    let mock_snap = mgr.mock_set_branding_tier(3, None).unwrap();
    assert_eq!(mock_snap.branding_tier, 3);
    assert!(mock_snap.is_private_market);
    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.branding_tier == 3 && s.market_logo_file.is_some()
        }),
        "Developer Mock Tier 3 should generate a cached DDS logo automatically"
    );

    mgr.shutdown();
    stop_server.store(true, Ordering::SeqCst);
    let _ = server_thread.join();

    // Cached DDS files in logos/ must survive client shutdown for instant reuse on next launch
    assert!(is_valid_cached_logo_dds(&dds_path_a));
    assert!(is_valid_cached_logo_dds(&dds_path_b));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn os_token_vault_dpapi_migration_and_write_only_security_work() {
    // 1. OS seal/unseal roundtrip & tamper rejection
    let sample_secret = b"{\"version\":1,\"entries\":[{\"raw_token\":\"gm_top_secret_token_9876\"}]}";
    let sealed = seal_vault_bytes(sample_secret).expect("OS vault seal must succeed silently");
    #[cfg(target_os = "windows")]
    {
        assert!(
            sealed.starts_with(b"GMV1WIN\0"),
            "Windows vault must use DPAPI GMV1WIN header"
        );
        let contains_plaintext = sealed
            .windows("gm_top_secret_token_9876".len())
            .any(|w| w == b"gm_top_secret_token_9876");
        assert!(
            !contains_plaintext,
            "Windows DPAPI ciphertext must never contain plaintext token bytes"
        );
    }

    let unsealed = unseal_vault_bytes(&sealed).expect("OS vault unseal must succeed");
    assert_eq!(unsealed, sample_secret);

    let mut tampered = sealed.clone();
    let last_idx = tampered.len() - 1;
    tampered[last_idx] ^= 0x5A;
    assert!(
        unseal_vault_bytes(&tampered).is_err(),
        "Tampered vault file must be rejected by OS integrity/DPAPI check"
    );

    // 2. Crash-safe automatic migration from legacy settings.json -> tokens.vault
    let dir = env::temp_dir().join(format!("fs25_gm_vault_mig_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    let cfg_file = dir.join("settings.json");
    let vault_file = TokenVault::vault_path_for_config(&cfg_file);
    let legacy_secret = "gm_legacy_secret_key_4321";
    let expected_legacy_id = compute_token_profile_id(legacy_secret);

    let legacy_json = serde_json::json!({
        "language": "de",
        "backendMode": "mock",
        "marketToken": legacy_secret
    });
    fs::write(&cfg_file, serde_json::to_string_pretty(&legacy_json).unwrap()).unwrap();

    let mgr = BridgeManager::new(cfg_file.clone(), None);
    assert!(
        vault_file.is_file(),
        "Legacy marketToken must be migrated into encrypted tokens.vault on startup"
    );

    let rewritten_settings = fs::read_to_string(&cfg_file).unwrap();
    assert!(
        !rewritten_settings.contains(legacy_secret),
        "Rewritten settings.json must NEVER contain the raw token string"
    );
    assert!(
        !rewritten_settings.contains("marketToken"),
        "Rewritten settings.json must omit deprecated marketToken field"
    );
    assert!(
        rewritten_settings.contains(&expected_legacy_id),
        "Rewritten settings.json must store only activeTokenId"
    );

    #[cfg(target_os = "windows")]
    {
        let raw_vault_on_disk = fs::read(&vault_file).unwrap();
        let leaked_on_disk = raw_vault_on_disk
            .windows(legacy_secret.len())
            .any(|w| w == legacy_secret.as_bytes());
        assert!(
            !leaked_on_disk,
            "Encrypted tokens.vault on Windows must not expose legacy secret in plaintext"
        );
    }

    // 3. Strict Write-Only IPC & Diagnostic Redaction guarantees
    let second_secret = "gm_second_private_token_abcd";
    let snap = mgr
        .add_market_token(second_secret, Some("Zweiter Hof"))
        .unwrap();
    assert_eq!(snap.saved_tokens.len(), 2);

    let serialized_cfg = serde_json::to_string(&mgr.get_config()).unwrap();
    let serialized_snap = serde_json::to_string(&snap).unwrap();
    let diag_report = mgr.generate_diagnostic_report();

    for secret in [legacy_secret, second_secret] {
        assert!(
            !serialized_cfg.contains(secret),
            "get_app_config IPC JSON must never serialize raw token"
        );
        assert!(
            !serialized_snap.contains(secret),
            "get_bridge_snapshot IPC JSON must never serialize raw token"
        );
        assert!(
            !diag_report.contains(secret),
            "Diagnostic report must never leak any vaulted token"
        );
    }
    assert!(serialized_snap.contains("••••4321"));
    assert!(serialized_snap.contains("••••abcd"));

    // 4. Label update + removal of all tokens cleans up tokens.vault from disk
    let second_id = compute_token_profile_id(second_secret);
    let updated_snap = mgr
        .update_market_token_label(&second_id, Some("Umbenannter Hof"))
        .unwrap();
    assert_eq!(
        updated_snap
            .saved_tokens
            .iter()
            .find(|p| p.id == second_id)
            .and_then(|p| p.custom_label.as_deref()),
        Some("Umbenannter Hof")
    );

    let after_rm_active = mgr.remove_market_token(&second_id).unwrap();
    assert!(
        after_rm_active.active_token_id.is_none(),
        "Removing the active token must fall back cleanly to Official Market (None)"
    );
    assert_eq!(after_rm_active.saved_tokens.len(), 1);

    let after_rm_all = mgr.remove_market_token(&expected_legacy_id).unwrap();
    assert!(after_rm_all.saved_tokens.is_empty());
    assert!(
        !vault_file.exists(),
        "Removing the last saved token must remove tokens.vault cleanly"
    );

    mgr.shutdown();
    let _ = fs::remove_dir_all(&dir);
}
