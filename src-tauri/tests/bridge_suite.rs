//! Comprehensive integration & unit test suite for the FS25 GlobalMarket Rust Core (Phases 2–4 & 3.5).

use globalmarket_client_lib::api::fastapi::{
    derive_handshake_branding, dev_token_allowed, parse_maintenance_error, resolve_safe_logo_url,
    FastApiBackend,
};
use globalmarket_client_lib::api::mock::LocalMockBackend;
use globalmarket_client_lib::api::types::{
    normalize_server_env, ContractActionRequest, ContractActionResultDto, ContractSettlementDto, ContractStateResponse,
    ContractSubAction, FillTypeStatus, LoadedMap, MarketItemDto, OnlineContractDto, OnlineDirectOfferDto,
    ServerNotice, ServerSecret, TransactionRequest, TransactionType, WhitelistItemDto,
    WhitelistSyncRequest,
};
use globalmarket_client_lib::api::{ApiError, BackendService};
use globalmarket_client_lib::bridge::coordinator::{
    encode_base64, format_unix_utc, BridgeConnectionState, BridgeManager,
};
use globalmarket_client_lib::bridge::identity::{ServerIdentityState, ServerIdentityStore};
use globalmarket_client_lib::bridge::notice::{
    classify_outage, message_languages, notice_for_maintenance_error, OfflineReason,
    StoredServerNotice,
};
use globalmarket_client_lib::bridge::dds::{
    convert_logo_bytes_to_dds_bytes, decode_dds_to_rgba8, default_dsa_logo_rel_path,
    is_valid_cached_logo_dds, logo_cache_hash12, logo_rel_path_for_hash, EXPECTED_LOGO_DDS_BYTES,
    LOGO_CANVAS_HEIGHT, LOGO_CANVAS_WIDTH, LOGO_MIPMAP_COUNT,
};
use globalmarket_client_lib::bridge::l10n::localize_fill_type;
use globalmarket_client_lib::bridge::paths::resolve_bridge_paths;
use globalmarket_client_lib::bridge::xml::{
    is_version_sufficient, parse_incoming_write_xml, parse_mod_info_map, read_mod_info_raw,
    remove_heartbeat_xml, sanitize_logo_rel_path,
    sanitize_market_name, write_contract_data_xml, write_contract_data_xml_with_identity,
    write_contract_results_xml, write_heartbeat_xml,
    write_heartbeat_xml_with_capabilities, write_heartbeat_xml_with_latest,
    write_heartbeat_xml_with_extras, write_heartbeat_xml_with_market_profiles,
    write_market_data_xml, write_market_data_xml_with_branding, HeartbeatBranding,
    HeartbeatExtras, HeartbeatMarketProfile, HeartbeatMarketProfiles, HeartbeatNotice,
    IncomingPacket, MAX_MARKET_NAME_BYTES, MAX_NOTICE_MESSAGE_BYTES,
    MAX_RETAINED_MARKET_SYNC_FILES,
};
use globalmarket_client_lib::config::vault::{
    compute_token_profile_id, seal_vault_bytes, unseal_vault_bytes, TokenVault,
};
use globalmarket_client_lib::config::{
    digest_access_key, languages_equivalent, normalize_language, normalize_theme,
    qualify_transaction_id, verify_access_key, verify_access_key_with_digests, AccessTier,
    AppConfig, BackendMode, DEFAULT_OFFICIAL_BACKEND_URL, OFFICIAL_DEV_BACKEND_URL,
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
use std::sync::{Arc, Mutex};
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
    assert!(!online_str.contains("<latestVersion>"));

    write_heartbeat_xml_with_latest(
        &staging,
        &online_file,
        true,
        "0.9.9",
        "0.9.9",
        Some("0.9.10"),
        None,
    )
    .unwrap();
    let online_with_latest = fs::read_to_string(&online_file).unwrap();
    assert!(online_with_latest.contains("<version>0.9.9</version>"));
    assert!(online_with_latest.contains("<minVersion>0.9.9</minVersion>"));
    assert!(online_with_latest.contains("<latestVersion>0.9.10</latestVersion>"));

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
fn parses_ingame_market_switch_requests() {
    let to_profile = r#"<gmDataSync action="selectMarket" profileId=" a1b2c3d4e5f6 " />"#;
    assert!(matches!(
        parse_incoming_write_xml(to_profile, "marketSwitch_20261123_140000_1").unwrap(),
        IncomingPacket::SelectMarket(Some(id)) if id == "a1b2c3d4e5f6"
    ));

    let to_official = r#"<gmDataSync action="selectMarket" profileId="" />"#;
    assert!(matches!(
        parse_incoming_write_xml(to_official, "marketSwitch_20261123_140000_2").unwrap(),
        IncomingPacket::SelectMarket(None)
    ));
}

#[test]
fn heartbeat_xml_exports_market_profiles_only_while_connected() {
    let dir = env::temp_dir().join(format!("fs25_gm_profiles_xml_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let staging = dir.join(".staging");
    let online_file = dir.join("globalMarketOnline.xml");

    let market = HeartbeatMarketProfiles {
        active_id: Some("a1b2c3d4e5f6".to_string()),
        profiles: vec![
            HeartbeatMarketProfile {
                id: "a1b2c3d4e5f6".to_string(),
                label: Some("Wochenend <Server>".to_string()),
                market_name: Some("WeekendFarmers".to_string()),
                branding_tier: Some(2),
            },
            HeartbeatMarketProfile {
                id: "0f9e8d7c6b5a".to_string(),
                label: None,
                market_name: None,
                branding_tier: None,
            },
        ],
    };

    write_heartbeat_xml_with_market_profiles(
        &staging,
        &online_file,
        true,
        "1.0.0",
        "1.0.0",
        None,
        None,
        true,
        Some(&market),
    )
    .unwrap();
    let xml = fs::read_to_string(&online_file).unwrap();
    assert!(xml.contains("marketSwitchSupported=\"true\""));
    assert!(xml.contains("<marketProfiles activeId=\"a1b2c3d4e5f6\">"));
    assert!(xml.contains(
        "<profile id=\"a1b2c3d4e5f6\" label=\"Wochenend &lt;Server&gt;\" marketName=\"WeekendFarmers\" brandingTier=\"2\" />"
    ));
    assert!(xml.contains("<profile id=\"0f9e8d7c6b5a\" />"));
    assert!(!xml.contains("masked"));

    // Disconnected: neither the capability flag nor the profile list
    write_heartbeat_xml_with_market_profiles(
        &staging,
        &online_file,
        false,
        "1.0.0",
        "1.0.0",
        None,
        None,
        true,
        Some(&market),
    )
    .unwrap();
    let offline_xml = fs::read_to_string(&online_file).unwrap();
    assert!(!offline_xml.contains("marketSwitchSupported"));
    assert!(!offline_xml.contains("<marketProfiles"));

    // Older call path stays unchanged
    write_heartbeat_xml_with_capabilities(
        &staging,
        &online_file,
        true,
        "1.0.0",
        "1.0.0",
        None,
        None,
        true,
    )
    .unwrap();
    assert!(!fs::read_to_string(&online_file)
        .unwrap()
        .contains("marketSwitchSupported"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn version_comparison_matches_lua_semantics() {
    // 3-part standard SemVer, 0.9.9 Beta lock, and 4-part legacy compatibility
    assert!(is_version_sufficient("0.9.9", "0.9.9"));
    assert!(is_version_sufficient("0.9.10", "0.9.9"));
    assert!(!is_version_sufficient("0.9.9", "1.0.0"));
    assert!(is_version_sufficient("1.0.0", "0.9.9"));
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

    for _ in 0..100 {
        let remaining_xmls: usize = fs::read_dir(&write_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "xml"))
            .count();
        let has_read_xml = fs::read_dir(&read_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .any(|e| e.path().extension().is_some_and(|ext| ext == "xml"))
            })
            .unwrap_or(false);
        let booked_logged = manager
            .get_logs()
            .iter()
            .any(|l| l.key.as_deref() == Some("logTransactionBooked"));
        if remaining_xmls == 0 && has_read_xml && booked_logged {
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
        connection_enabled: true,
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
        exe_version: "0.9.17".to_string(),
        min_exe_version: "0.9.9".to_string(),
        latest_exe_version: Some("0.9.18".to_string()),
        contracts_supported: true,
        open_contracts_count: 2,
        open_offers_count: 3,
        vehicles_supported: true,
        open_vehicle_listings_count: 4,
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
        extended_expires_unix: None,
        server_notice: Some(ServerNotice {
            id: "2025-12-01-maintenance".to_string(),
            notice_type: "maintenance".to_string(),
            starts_at: Some(1_764_621_000),
            ends_at: Some(1_764_624_600),
            message: Default::default(),
        }),
        offline_reason: Some("maintenance".to_string()),
        server_identity: "unknown".to_string(),
        has_dev_token: true,
        server_env: Some("development".to_string()),
        loaded_map: Some("FS25_ExampleMap.ExampleMap (FS25_ExampleMap, hash 8b1d07aa)".to_string()),
        unsupported_content_label: None,
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
    assert!(report.contains("app_version: \"0.9.17-beta (protocol 0.9.17)\""));
    assert!(report.contains("connection_state: \"ONLINE (min_exe_version=0.9.9, latest_exe_version=0.9.18)\""));
    assert!(report.contains("contracts_bridge: \"supported=true, open_contracts=2, open_offers=3\""));
    assert!(report.contains("vehicles_bridge: \"supported=true, open_listings=4\""));
    assert!(report.contains("client_slot_prefix: \"a3f901…\""));
    assert!(report.contains("lifebit_seq=7"));
    assert!(report.contains("configured (redacted)"));
    assert!(report.contains("game_edition: \"FS25\""));
    assert!(report.contains("tier=developer"));
    assert!(report.contains("approved=10, pending=2, rejected=0"));
    assert!(report.contains(
        "server_notice: \"id=2025-12-01-maintenance, type=maintenance, starts=2025-12-01 20:30 UTC, ends=2025-12-01 21:30 UTC, offline_reason=maintenance\""
    ));
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
        // Serve up to 11 sequential HTTP requests on 127.0.0.1 with a bounded deadline
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        for step in 0..11 {
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
                    let json = r#"{"online":true,"min_exe_version":"0.9.9","recommended_exe_version":"1.0.2","poll_interval_sec":10,"market_name":"DSA Community Market","api_version":"2.0.0","market_logo_url":"/assets/community-logo.png"}"#;
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
                // 8: GET /api/v1/meta/status -> 403 CLIENT_IP_LOCKED (JSON retry_after + Retry-After header)
                8 => {
                    assert!(request_line.starts_with("GET /api/v1/meta/status "));
                    let json = r#"{"detail":{"code":"CLIENT_IP_LOCKED","retry_after":2400}}"#;
                    format!(
                        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nRetry-After: 2400\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                // 9: GET /api/v1/meta/status -> 403 CLIENT_IP_LOCKED (header-only Retry-After fallback)
                9 => {
                    assert!(request_line.starts_with("GET /api/v1/meta/status "));
                    let json = r#"{"detail":{"code":"CLIENT_IP_LOCKED"}}"#;
                    format!(
                        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nRetry-After: 180\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                // 10: GET /api/v1/meta/status -> 403 CLIENT_ID_REQUIRED
                10 => {
                    assert!(request_line.starts_with("GET /api/v1/meta/status "));
                    let json = r#"{"detail":{"code":"CLIENT_ID_REQUIRED"}}"#;
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
    assert_eq!(hs.min_exe_version, "0.9.9");
    assert_eq!(hs.latest_exe_version.as_deref(), Some("1.0.2"));
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

    // Step 8, 9 & 10: Key-slot IP binding (`CLIENT_IP_LOCKED`) & client ID validation (`CLIENT_ID_REQUIRED`)
    let ip_locked_err = client.check_status().unwrap_err();
    assert!(matches!(
        ip_locked_err,
        ApiError::ClientIpLocked {
            retry_after_sec: 2400
        }
    ));

    let ip_locked_hdr_err = client.check_status().unwrap_err();
    assert!(matches!(
        ip_locked_hdr_err,
        ApiError::ClientIpLocked {
            retry_after_sec: 180
        }
    ));

    let cid_req_err = client.check_status().unwrap_err();
    assert!(matches!(cid_req_err, ApiError::ClientIdRequired));

    server_thread.join().unwrap();
    assert_eq!(served_requests.load(Ordering::SeqCst), 11);
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

    // 16-language basegame & production fillType translation (sourced from official FS25 l10n)
    assert_eq!(localize_fill_type("WHEAT", "de"), Some("Weizen"));
    assert_eq!(localize_fill_type("WHEAT", "en"), Some("Wheat"));
    assert_eq!(localize_fill_type("WHEAT", "fr"), Some("Blé"));
    assert_eq!(localize_fill_type("WHEAT", "es"), Some("Trigo"));
    assert_eq!(localize_fill_type("WHEAT", "pl"), Some("Pszenica"));
    assert_eq!(localize_fill_type("WHEAT", "it"), Some("Frumento"));
    assert_eq!(localize_fill_type("WHEAT", "nl"), Some("Tarwe"));
    assert_eq!(localize_fill_type("WHEAT", "cz"), Some("Pšenice"));
    assert_eq!(localize_fill_type("WHEAT", "cs-CZ"), Some("Pšenice"));
    assert_eq!(localize_fill_type("WHEAT", "pt"), Some("Trigo"));
    assert_eq!(localize_fill_type("WHEAT", "br"), Some("Trigo"));
    assert_eq!(localize_fill_type("WHEAT", "hu"), Some("Búza"));
    assert_eq!(localize_fill_type("WHEAT", "da"), Some("Hvede"));
    assert_eq!(localize_fill_type("WHEAT", "sv"), Some("Vete"));
    assert_eq!(localize_fill_type("WHEAT", "ro"), Some("Grâu"));
    assert_eq!(localize_fill_type("WHEAT", "tr"), Some("Buğday"));
    assert_eq!(localize_fill_type("WHEAT", "ru"), Some("Пшеница"));
    assert_eq!(localize_fill_type("WHEAT", "uk"), Some("Пшениця"));

    // Language normalization & GIANTS/ISO code equivalence
    for code in [
        "de", "en", "fr", "es", "pl", "it", "nl", "cz", "pt", "hu", "da", "sv", "ro", "tr", "ru",
        "uk",
    ] {
        assert_eq!(normalize_language(code), code);
    }
    assert_eq!(normalize_language("cs-CZ"), "cz");
    assert_eq!(normalize_language("pt-BR"), "pt");
    assert_eq!(normalize_language("br"), "pt");
    assert_eq!(normalize_language("ua"), "uk");
    assert_eq!(normalize_language("xx-UNKNOWN"), "en");
    assert!(languages_equivalent("br", "pt"));
    assert!(languages_equivalent("cs", "cz"));
    assert!(!languages_equivalent("jp", "en"));

    // Theme normalization (defaults strictly to dark)
    assert_eq!(AppConfig::default().theme, "dark");
    assert_eq!(normalize_theme("dark"), "dark");
    assert_eq!(normalize_theme("LIGHT"), "light");
    assert_eq!(normalize_theme("system"), "system");
    assert_eq!(normalize_theme("auto"), "system");
    assert_eq!(normalize_theme("invalid-theme"), "dark");

    assert_eq!(localize_fill_type("RAISINS", "de"), Some("Rosinen"));
    assert_eq!(localize_fill_type("RAISINS", "en"), Some("Raisins"));
    assert_eq!(localize_fill_type("RAISINS", "fr"), Some("Raisins secs"));
    assert_eq!(localize_fill_type("RAISINS", "es"), Some("Pasas"));
    assert_eq!(localize_fill_type("RAISINS", "pl"), Some("Rodzynki"));

    assert_eq!(localize_fill_type("RICELONGGRAIN", "de"), Some("Langkornreis"));
    assert_eq!(localize_fill_type("PEA", "de"), Some("Erbsen"));
    assert_eq!(localize_fill_type("GREENBEAN", "de"), Some("Buschbohnen"));
    assert_eq!(localize_fill_type("FERMENTEDNAPACABBAGE", "de"), Some("Kimchi"));
    assert_eq!(localize_fill_type("BUFFALOMOZZARELLA", "de"), Some("Büffelmozzarella"));
    assert_eq!(localize_fill_type("JARRED_GREENBEAN", "de"), Some("Eingemachte Buschbohnen"));
    assert_eq!(localize_fill_type("ONION", "de"), Some("Zwiebeln"));
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
        derive_handshake_branding(
            true,
            "Global Market Official",
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
        ),
        "Server PUBLIC_MARKET_NAME ('Global Market Official') with a logo-only key must resolve to Tier 1 Official Market"
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

/// Returns the `<globalMarketOnline ...>` start tag of a heartbeat file (root attributes only).
fn heartbeat_root(xml: &str) -> String {
    xml.find("<globalMarketOnline")
        .and_then(|start| {
            xml[start..]
                .find('>')
                .map(|end| xml[start..=start + end].to_string())
        })
        .unwrap_or_default()
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
                                "{{\"status\":\"online\",\"version\":\"2.1.0\",\"min_exe_version\":\"0.9.9\",\"latest_exe_version\":\"0.9.18\",\"branding_tier\":3,\"is_private_market\":true,\"market_name\":\"LSMT Community Markt\",\"has_custom_logo\":true,\"market_logo_url\":\"http://{addr}/logos/lsmt.webp\"}}"
                            ),
                            Some("token-bravo-tier1") => format!(
                                "{{\"status\":\"online\",\"version\":\"2.1.0\",\"min_exe_version\":\"0.9.9\",\"branding_tier\":1,\"is_private_market\":false,\"market_name\":\"Global Market\",\"has_custom_logo\":true,\"market_logo_url\":\"http://{addr}/logos/tier1.webp\"}}"
                            ),
                            _ => "{\"status\":\"online\",\"version\":\"2.1.0\",\"min_exe_version\":\"0.9.9\",\"branding_tier\":0,\"is_private_market\":false,\"market_name\":\"Global Market\",\"has_custom_logo\":false}".to_string(),
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
    assert!(xml_a.contains("<version>0.9.17</version>"));
    assert!(xml_a.contains("<minVersion>0.9.9</minVersion>"));
    assert!(xml_a.contains("<latestVersion>0.9.18</latestVersion>"));
    assert_eq!(snap_after_a.latest_exe_version.as_deref(), Some("0.9.18"));
    assert!(mgr
        .get_logs()
        .iter()
        .any(|l| l.key.as_deref() == Some("logOptionalUpdateAvailable")));

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
    // Branding lives on the root element; `<marketProfiles>` may list other profiles' market names.
    let root_b = heartbeat_root(&xml_b);
    assert!(root_b.contains("brandingTier=\"1\""));
    assert!(root_b.contains("isPrivateMarket=\"false\""));
    assert!(!root_b.contains("marketName="));
    assert!(root_b.contains("hasCustomLogo=\"true\""));
    assert!(root_b.contains(&format!("logoFile=\"{expected_rel_b}\"")));
    assert!(!xml_b.contains("<latestVersion>"));

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

    // 4. Switch to Official Market (None / UI Tier 0) while keeping both tokens saved in vault!
    // Built-in DSA logo fallback guarantees FS25 always renders the DSA `.dds` logo in-game (`brandingTier="1"`).
    let default_dsa_rel = default_dsa_logo_rel_path();
    mgr.select_active_market_token(None).unwrap();

    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && s.branding_tier == 0
                && s.active_token_id.is_none()
                && s.saved_tokens.len() == 2
                && !s.is_private_market
                && s.market_logo_file.as_deref() == Some(default_dsa_rel.as_str())
                && fs::read_to_string(&online_xml_path)
                    .map(|x| {
                        let root = heartbeat_root(&x);
                        root.contains("brandingTier=\"1\"")
                            && root.contains("isPrivateMarket=\"false\"")
                            && !root.contains("marketName=")
                            && root.contains("hasCustomLogo=\"true\"")
                            && root.contains(&format!("logoFile=\"{default_dsa_rel}\""))
                    })
                    .unwrap_or(false)
        }),
        "Switching to Official Market should keep UI branding_tier=0 while emitting the built-in DSA DDS logo for FS25"
    );
    assert!(is_valid_cached_logo_dds(&bridge_root.join(&default_dsa_rel)));

    // 4b. Players can switch the official DSA logo off: no logo for FS25, name and status unchanged.
    let mut cfg_no_logo = mgr.get_config();
    cfg_no_logo.show_official_logo = false;
    mgr.update_config(cfg_no_logo).unwrap();
    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && s.market_logo_file.is_none()
                && fs::read_to_string(&online_xml_path)
                    .map(|x| {
                        let root = heartbeat_root(&x);
                        root.contains("brandingTier=\"0\"")
                            && root.contains("isConnected=\"true\"")
                            && !root.contains("logoFile=")
                    })
                    .unwrap_or(false)
        }),
        "Disabling the official logo should drop the DSA logoFile from the heartbeat without a reconnect"
    );
    let mut cfg_logo = mgr.get_config();
    cfg_logo.show_official_logo = true;
    mgr.update_config(cfg_logo).unwrap();
    assert!(
        wait_for(&|| {
            mgr.get_snapshot().market_logo_file.as_deref() == Some(default_dsa_rel.as_str())
        }),
        "Re-enabling the official logo should restore the built-in DSA logo"
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

#[test]
fn client_ip_locked_pauses_polling_and_unlocks_immediately_on_market_switch() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    let locked_hits = Arc::new(AtomicUsize::new(0));
    let public_hits = Arc::new(AtomicUsize::new(0));
    let stop_srv = Arc::new(AtomicBool::new(false));

    let locked_hits_srv = Arc::clone(&locked_hits);
    let public_hits_srv = Arc::clone(&public_hits);
    let stop_srv_clone = Arc::clone(&stop_srv);

    let server_thread = thread::spawn(move || {
        while !stop_srv_clone.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut reader = BufReader::new(&mut stream);
                    let mut req_line = String::new();
                    if reader.read_line(&mut req_line).is_err() || req_line.is_empty() {
                        continue;
                    }
                    let mut token_hdr: Option<String> = None;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty()
                        {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("x-market-token") {
                                token_hdr = Some(v.trim().to_string());
                            }
                        }
                    }
                    drop(reader);

                    if req_line.starts_with("GET /api/v1/meta/status ") {
                        if token_hdr.as_deref() == Some("locked-ip-token") {
                            locked_hits_srv.fetch_add(1, Ordering::SeqCst);
                            let json = r#"{"detail":{"code":"CLIENT_IP_LOCKED","retry_after":120}}"#;
                            let resp = format!(
                                "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nRetry-After: 120\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                json.len(),
                                json
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        } else {
                            public_hits_srv.fetch_add(1, Ordering::SeqCst);
                            let json = r#"{"status":"online","version":"2.4.0","min_exe_version":"0.9.9","market_name":"Global Market Official"}"#;
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                json.len(),
                                json
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        }
                    } else if req_line.starts_with("GET /api/v1/market/state ") {
                        let json = r#"{"updated_at":1763058000,"items":[]}"#;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            json.len(),
                            json
                        );
                        let _ = stream.write_all(resp.as_bytes());
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

    let dir = env::temp_dir().join(format!("fs25_gm_ip_lock_pause_{}", std::process::id()));
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

    mgr.add_market_token("locked-ip-token", Some("Gesperrtes Netz"))
        .unwrap();

    let mut cfg = mgr.get_config();
    cfg.backend_mode = BackendMode::Live;
    cfg.backend_url = format!("http://{addr}");
    cfg.fs25_profile_path = Some(profile_dir.to_string_lossy().into_owned());
    mgr.update_config(cfg).unwrap();

    let wait_for = |pred: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            thread::sleep(Duration::from_millis(40));
        }
        false
    };

    assert!(
        wait_for(&|| {
            mgr.get_logs()
                .iter()
                .any(|l| l.key.as_deref() == Some("logClientIpLocked"))
        }),
        "Worker must emit logClientIpLocked (not logTokenInvalid) on HTTP 403 CLIENT_IP_LOCKED"
    );
    assert!(!mgr
        .get_logs()
        .iter()
        .any(|l| l.key.as_deref() == Some("logTokenInvalid")));

    // Wait across multiple 500 ms worker ticks -> polling MUST remain paused (zero extra hits)!
    let hits_after_lock = locked_hits.load(Ordering::SeqCst);
    assert!(hits_after_lock >= 1);
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        locked_hits.load(Ordering::SeqCst),
        hits_after_lock,
        "Worker must pause polling during retry_after instead of retrying every tick"
    );

    // Switching back to the Official Market (`None`) must immediately clear the hold and connect
    mgr.select_active_market_token(None).unwrap();
    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && s.branding_tier == 0
                && !s.is_private_market
                && s.market_name == "Global Market"
        }),
        "Switching to Official Market must immediately clear IP-lock hold and normalize 'Global Market Official' to 'Global Market'"
    );
    assert!(public_hits.load(Ordering::SeqCst) >= 1);

    mgr.shutdown();
    stop_srv.store(true, Ordering::SeqCst);
    let _ = server_thread.join();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn live_market_switch_preserves_heartbeat_xml_during_handshake_and_deletes_on_shutdown() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    let stop_srv = Arc::new(AtomicBool::new(false));
    let stop_srv_clone = Arc::clone(&stop_srv);

    let server_thread = thread::spawn(move || {
        while !stop_srv_clone.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut reader = BufReader::new(&mut stream);
                    let mut req_line = String::new();
                    if reader.read_line(&mut req_line).is_err() || req_line.is_empty() {
                        continue;
                    }
                    let mut token_hdr: Option<String> = None;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty()
                        {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("x-market-token") {
                                token_hdr = Some(v.trim().to_string());
                            }
                        }
                    }
                    drop(reader);

                    if req_line.starts_with("GET /api/v1/meta/status ") {
                        if token_hdr.as_deref() == Some("slow-dedi-token") {
                            // Simulate a realistic 250 ms HTTPS handshake / status latency
                            thread::sleep(Duration::from_millis(250));
                            let json = r#"{"online":true,"min_exe_version":"0.9.9","poll_interval_sec":10,"market_name":"Dedi Community Markt","api_version":"2.6.0","market_logo_url":null}"#;
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                json.len(),
                                json
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        } else {
                            let json = r#"{"online":true,"min_exe_version":"0.9.9","poll_interval_sec":10,"market_name":"Global Market Official","api_version":"2.6.0","market_logo_url":null}"#;
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                json.len(),
                                json
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        }
                    } else if req_line.starts_with("GET /api/v1/market/state ") {
                        if token_hdr.as_deref() == Some("slow-dedi-token") {
                            let json = r#"{"updated_at":1763058010,"items":[{"fill_type":"BARLEY","fill_level":25000,"trend":1.05,"great_demand":false,"status":"APPROVED"}]}"#;
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                json.len(),
                                json
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        } else {
                            let json = r#"{"updated_at":1763058000,"items":[{"fill_type":"WHEAT","fill_level":50000,"trend":1.00,"great_demand":false,"status":"APPROVED"}]}"#;
                            let resp = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                json.len(),
                                json
                            );
                            let _ = stream.write_all(resp.as_bytes());
                        }
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

    let dir = env::temp_dir().join(format!("fs25_gm_restart_preserve_hb_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let profile_dir_1 = dir.join("FarmingSimulator2025_A");
    let profile_dir_2 = dir.join("FarmingSimulator2025_B");
    fs::create_dir_all(&profile_dir_1).unwrap();
    fs::create_dir_all(&profile_dir_2).unwrap();
    fs::write(profile_dir_1.join("gameSettings.xml"), "<gameSettings/>").unwrap();
    fs::write(profile_dir_2.join("gameSettings.xml"), "<gameSettings/>").unwrap();

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

    let mut cfg = mgr.get_config();
    cfg.backend_mode = BackendMode::Live;
    cfg.backend_url = format!("http://{addr}");
    cfg.fs25_profile_path = Some(profile_dir_1.to_string_lossy().into_owned());
    mgr.update_config(cfg).unwrap();

    let bridge_root_1 = profile_dir_1.join("modSettings").join("FS25_GlobalMarket");
    let online_xml_1 = bridge_root_1.join("globalMarketOnline.xml");
    let read_dir_1 = bridge_root_1.join("read");

    let wait_for = |pred: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        false
    };

    assert!(
        wait_for(&|| {
            mgr.get_snapshot().state == BridgeConnectionState::Online
                && online_xml_1.exists()
                && fs::read_dir(&read_dir_1)
                    .map(|it| it.flatten().count() > 0)
                    .unwrap_or(false)
        }),
        "Initial Official Market connection must write globalMarketOnline.xml and gmDataSync_*.xml"
    );

    // Spawn a high-frequency observer (every 10 ms) to verify `globalMarketOnline.xml` NEVER disappears
    // during a live token switch even while the new HTTPS handshake takes 250 ms!
    let saw_missing_heartbeat = Arc::new(AtomicBool::new(false));
    let stop_observer = Arc::new(AtomicBool::new(false));
    let obs_missing_clone = Arc::clone(&saw_missing_heartbeat);
    let obs_stop_clone = Arc::clone(&stop_observer);
    let obs_xml_path = online_xml_1.clone();
    let observer = thread::spawn(move || {
        while !obs_stop_clone.load(Ordering::SeqCst) {
            if !obs_xml_path.exists() {
                obs_missing_clone.store(true, Ordering::SeqCst);
            }
            thread::sleep(Duration::from_millis(10));
        }
    });

    // Switch live to `slow-dedi-token` (250 ms handshake)
    mgr.add_market_token("slow-dedi-token", Some("Dedi Projekt"))
        .unwrap();

    // Immediately after `add_market_token` returns (while the 250 ms handshake is still in flight!),
    // old `read/gmDataSync_*.xml` snapshots must already be purged while `globalMarketOnline.xml` still exists.
    assert!(
        online_xml_1.exists(),
        "globalMarketOnline.xml must remain intact immediately after live token switch"
    );
    let old_wheats_left = fs::read_dir(&read_dir_1)
        .map(|it| {
            it.flatten()
                .filter_map(|e| fs::read_to_string(e.path()).ok())
                .any(|c| c.contains("WHEAT"))
        })
        .unwrap_or(false);
    assert!(
        !old_wheats_left,
        "Unread gmDataSync_*.xml snapshots from the previous market must be cleared on live restart"
    );

    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && s.is_private_market
                && s.market_name == "Dedi Community Markt"
                && fs::read_to_string(&online_xml_1)
                    .map(|xml| xml.contains("marketName=\"Dedi Community Markt\""))
                    .unwrap_or(false)
        }),
        "New worker must atomically overwrite globalMarketOnline.xml with the new private market branding"
    );

    stop_observer.store(true, Ordering::SeqCst);
    let _ = observer.join();
    assert!(
        !saw_missing_heartbeat.load(Ordering::SeqCst),
        "globalMarketOnline.xml must NEVER be deleted during a live market/token switch on the same profile folder"
    );

    // Switching to a DIFFERENT FS25 profile folder MUST clean up `globalMarketOnline.xml` in the old folder
    mgr.set_profile_path(&profile_dir_2.to_string_lossy())
        .unwrap();
    let online_xml_2 = profile_dir_2
        .join("modSettings")
        .join("FS25_GlobalMarket")
        .join("globalMarketOnline.xml");
    assert!(
        !online_xml_1.exists(),
        "Changing to a different FS25 profile folder must remove globalMarketOnline.xml from the previous folder"
    );
    assert!(wait_for(&|| online_xml_2.exists()));

    // Real shutdown MUST immediately delete `globalMarketOnline.xml`
    mgr.shutdown();
    assert!(
        !online_xml_2.exists(),
        "Real shutdown() must immediately remove globalMarketOnline.xml"
    );

    stop_srv.store(true, Ordering::SeqCst);
    let _ = server_thread.join();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn manual_disconnect_switch_and_auto_connect_on_launch_false_work_cleanly() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    let stop_srv = Arc::new(AtomicBool::new(false));
    let stop_srv_clone = Arc::clone(&stop_srv);
    let meta_hits = Arc::new(AtomicUsize::new(0));
    let market_hits = Arc::new(AtomicUsize::new(0));
    let meta_hits_srv = Arc::clone(&meta_hits);
    let market_hits_srv = Arc::clone(&market_hits);

    let server_thread = thread::spawn(move || {
        while !stop_srv_clone.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut reader = BufReader::new(&mut stream);
                    let mut req_line = String::new();
                    if reader.read_line(&mut req_line).is_err() || req_line.is_empty() {
                        continue;
                    }
                    let mut token_hdr: Option<String> = None;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty()
                        {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("x-market-token") {
                                token_hdr = Some(v.trim().to_string());
                            }
                        }
                    }
                    drop(reader);

                    if req_line.starts_with("GET /api/v1/meta/status ") {
                        meta_hits_srv.fetch_add(1, Ordering::SeqCst);
                        let market_name = if token_hdr.as_deref() == Some("dedi-token") {
                            "Custom Dedi Hub"
                        } else {
                            "Global Market Official"
                        };
                        let json = format!(
                            r#"{{"online":true,"min_exe_version":"0.9.9","poll_interval_sec":10,"market_name":"{market_name}","api_version":"2.6.0","market_logo_url":null}}"#
                        );
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            json.len(),
                            json
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    } else if req_line.starts_with("GET /api/v1/market/state ") {
                        market_hits_srv.fetch_add(1, Ordering::SeqCst);
                        let json = r#"{"updated_at":1763058000,"items":[{"fill_type":"WHEAT","fill_level":50000,"trend":1.00,"great_demand":false,"status":"APPROVED"}]}"#;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            json.len(),
                            json
                        );
                        let _ = stream.write_all(resp.as_bytes());
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

    let dir = env::temp_dir().join(format!("fs25_gm_disconnect_toggle_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let profile_dir = dir.join("FarmingSimulator2025");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(profile_dir.join("gameSettings.xml"), "<gameSettings/>").unwrap();

    let paths = resolve_bridge_paths(&profile_dir).unwrap();
    fs::create_dir_all(&paths.read_dir).unwrap();
    fs::create_dir_all(&paths.write_dir).unwrap();
    fs::create_dir_all(&paths.staging_dir).unwrap();

    // Seed stale runtime XML artifacts before boot to verify auto_connect_on_launch=false cleans them up.
    fs::write(
        &paths.online_xml_file,
        "<globalMarketOnline isConnected=\"true\" protocolVersion=\"1\" />",
    )
    .unwrap();
    fs::write(
        paths.read_dir.join("gmDataSync_stale.xml"),
        "<gmDataSync action=\"marketData\" />",
    )
    .unwrap();
    fs::write(paths.staging_dir.join("stale.tmp"), "temp").unwrap();

    let cfg_file = dir.join("settings.json");
    let init_cfg = AppConfig {
        access_tier: AccessTier::Developer,
        backend_mode: BackendMode::Live,
        backend_url: format!("http://{addr}"),
        fs25_profile_path: Some(profile_dir.to_string_lossy().into_owned()),
        auto_connect_on_launch: false,
        ..AppConfig::default()
    };
    init_cfg.save_to_file(&cfg_file).unwrap();

    let mgr = BridgeManager::new(cfg_file, None);
    let initial_snap = mgr.get_snapshot();
    assert!(!mgr.is_connection_enabled());
    assert!(!initial_snap.connection_enabled);
    assert_eq!(initial_snap.state, BridgeConnectionState::Disconnected);
    assert!(
        !paths.online_xml_file.exists(),
        "Launching with auto_connect_on_launch=false must delete stale globalMarketOnline.xml"
    );
    assert_eq!(fs::read_dir(&paths.read_dir).unwrap().flatten().count(), 0);
    assert_eq!(fs::read_dir(&paths.staging_dir).unwrap().flatten().count(), 0);

    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        meta_hits.load(Ordering::SeqCst),
        0,
        "Disconnected startup must make zero status HTTP requests"
    );
    assert_eq!(
        market_hits.load(Ordering::SeqCst),
        0,
        "Disconnected startup must make zero market HTTP requests"
    );

    // Switching/adding a market token while disconnected must remain Disconnected without polling.
    mgr.add_market_token("dedi-token", Some("My Dedi")).unwrap();
    thread::sleep(Duration::from_millis(200));
    let after_token_snap = mgr.get_snapshot();
    assert!(!mgr.is_connection_enabled());
    assert_eq!(after_token_snap.state, BridgeConnectionState::Disconnected);
    assert_eq!(meta_hits.load(Ordering::SeqCst), 0);
    assert!(!paths.online_xml_file.exists());

    // Connecting manually enables worker + HTTP polling + XML generation with branding attributes.
    let conn_snap = mgr.set_connection_enabled(true);
    assert!(conn_snap.connection_enabled);
    assert!(mgr.is_connection_enabled());

    let wait_for = |pred: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        false
    };

    assert!(
        wait_for(&|| {
            let s = mgr.get_snapshot();
            s.state == BridgeConnectionState::Online
                && paths.online_xml_file.exists()
                && fs::read_dir(&paths.read_dir)
                    .map(|it| it.flatten().count() > 0)
                    .unwrap_or(false)
        }),
        "Manual connect must bring the bridge online and emit heartbeat + gmDataSync XMLs"
    );
    assert!(meta_hits.load(Ordering::SeqCst) > 0);
    assert!(market_hits.load(Ordering::SeqCst) > 0);

    let default_dsa_rel = default_dsa_logo_rel_path();
    assert_eq!(mgr.get_snapshot().branding_tier, 2);
    let sync_xml_content = fs::read_dir(&paths.read_dir)
        .unwrap()
        .flatten()
        .find_map(|e| fs::read_to_string(e.path()).ok())
        .expect("Expected gmDataSync XML in read/");
    assert!(
        sync_xml_content.contains("isPrivateMarket=\"true\"")
            && sync_xml_content.contains("marketName=\"Custom Dedi Hub\"")
            && sync_xml_content.contains("brandingTier=\"3\"")
            && sync_xml_content.contains(&format!("logoFile=\"{default_dsa_rel}\"")),
        "gmDataSync XML for Private Market without custom logo must emit built-in DSA DDS logo: {sync_xml_content}"
    );
    assert!(is_valid_cached_logo_dds(&paths.bridge_root.join(&default_dsa_rel)));

    // Seed an extra staging temp file, then disconnect manually: cleanup must be immediate and HTTP traffic must stop.
    fs::write(paths.staging_dir.join("in_flight.tmp"), "partial").unwrap();
    let disc_snap = mgr.set_connection_enabled(false);
    assert!(!disc_snap.connection_enabled);
    assert!(!mgr.is_connection_enabled());
    assert_eq!(disc_snap.state, BridgeConnectionState::Disconnected);
    assert!(
        !paths.online_xml_file.exists(),
        "Manual disconnect must immediately delete globalMarketOnline.xml"
    );
    assert_eq!(
        fs::read_dir(&paths.read_dir).unwrap().flatten().count(),
        0,
        "Manual disconnect must immediately clear unread gmDataSync_*.xml files"
    );
    assert_eq!(
        fs::read_dir(&paths.staging_dir).unwrap().flatten().count(),
        0,
        "Manual disconnect must immediately clear staging temp files"
    );

    let hits_at_disconnect =
        meta_hits.load(Ordering::SeqCst) + market_hits.load(Ordering::SeqCst);
    thread::sleep(Duration::from_millis(350));
    assert_eq!(
        meta_hits.load(Ordering::SeqCst) + market_hits.load(Ordering::SeqCst),
        hits_at_disconnect,
        "Disconnected state must produce zero HTTP requests after worker stops"
    );

    mgr.shutdown();
    stop_srv.store(true, Ordering::SeqCst);
    let _ = server_thread.join();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn market_data_xml_with_branding_stamps_tier_and_logo_attributes() {
    let dir = env::temp_dir().join(format!("fs25_gm_sync_branding_xml_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let staging_dir = dir.join(".staging");
    let read_dir = dir.join("read");
    fs::create_dir_all(&staging_dir).unwrap();
    fs::create_dir_all(&read_dir).unwrap();

    let items = vec![MarketItemDto {
        fill_type: "WHEAT".to_string(),
        title: Some("Wheat".to_string()),
        fill_level: 10_000,
        trend: 1.0,
        great_demand: false,
        great_demand_bonus: 0.0,
        great_demand_remaining: 0,
        status: FillTypeStatus::Approved,
    }];

    // Tier 3 private market with custom DDS logo
    let tier3 = HeartbeatBranding {
        branding_tier: 3,
        is_private_market: true,
        market_name: Some("Nordic Coop".to_string()),
        has_custom_logo: true,
        logo_file: Some("logos/logo_abc123def456.dds".to_string()),
    };
    let path_t3 =
        write_market_data_xml_with_branding(&staging_dir, &read_dir, &items, Some(&tier3)).unwrap();
    let xml_t3 = fs::read_to_string(&path_t3).unwrap();
    assert!(xml_t3.contains("brandingTier=\"3\""));
    assert!(xml_t3.contains("isPrivateMarket=\"true\""));
    assert!(xml_t3.contains("marketName=\"Nordic Coop\""));
    assert!(xml_t3.contains("logoFile=\"logos/logo_abc123def456.dds\""));

    // Tier 0 public market without custom logo: explicit empty logoFile="" clears Lua cache
    let tier0 = HeartbeatBranding {
        branding_tier: 0,
        is_private_market: false,
        market_name: None,
        has_custom_logo: false,
        logo_file: None,
    };
    let path_t0 =
        write_market_data_xml_with_branding(&staging_dir, &read_dir, &items, Some(&tier0)).unwrap();
    let xml_t0 = fs::read_to_string(&path_t0).unwrap();
    assert!(xml_t0.contains("brandingTier=\"0\""));
    assert!(xml_t0.contains("isPrivateMarket=\"false\""));
    assert!(xml_t0.contains("logoFile=\"\""));
    assert!(!xml_t0.contains("marketName="));

    // None branding remains clean for legacy callers
    let path_none =
        write_market_data_xml_with_branding(&staging_dir, &read_dir, &items, None).unwrap();
    let xml_none = fs::read_to_string(&path_none).unwrap();
    assert!(!xml_none.contains("brandingTier="));
    assert!(!xml_none.contains("logoFile="));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stage2_contract_xml_roundtrip_and_pruning_isolation() {
    let dir = env::temp_dir().join(format!("fs25_gm_contracts_xml_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let staging_dir = dir.join(".staging");
    let read_dir = dir.join("read");
    let online_file = dir.join("globalMarketOnline.xml");

    // 1. Heartbeat contractsSupported capability gate (true only when connected && supported)
    write_heartbeat_xml_with_capabilities(
        &staging_dir,
        &online_file,
        true,
        "0.9.12",
        "0.9.10",
        None,
        None,
        true,
    )
    .unwrap();
    let hb_on = fs::read_to_string(&online_file).unwrap();
    assert!(hb_on.contains("isConnected=\"true\""));
    assert!(hb_on.contains("contractsSupported=\"true\""));

    write_heartbeat_xml_with_capabilities(
        &staging_dir,
        &online_file,
        false,
        "0.9.12",
        "0.9.10",
        None,
        None,
        true,
    )
    .unwrap();
    let hb_off = fs::read_to_string(&online_file).unwrap();
    assert!(hb_off.contains("isConnected=\"false\""));
    assert!(hb_off.contains("contractsSupported=\"false\""));

    // 2. Outgoing contractAction XML parsing (including negative priceDeltaPct, bales/pallets, ACK_SETTLEMENT)
    let outgoing_xml = r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="contractAction">
    <request subAction="CREATE_OFFER" actionId="a3f901c48b2e7d10_12000_1" serverId="a3f901c48b2e7d10" farmId="2" farmName="Südhof &amp; Co" fillType="barley" liters="25000" priceDeltaPct="-7" allowPartial="false" durationHours="48" wasBale="true" wasPallet="false" />
    <request subAction="ACK_SETTLEMENT" actionId="a3f901c48b2e7d10_12050_2" serverId="a3f901c48b2e7d10" settlementIds="set_901,set_902" />
</gmDataSync>"#;

    match parse_incoming_write_xml(outgoing_xml, "20261117_200000_1").unwrap() {
        IncomingPacket::ContractActions(reqs) => {
            assert_eq!(reqs.len(), 2);
            assert_eq!(reqs[0].sub_action, ContractSubAction::CreateOffer);
            assert_eq!(reqs[0].action_id, "a3f901c48b2e7d10_12000_1");
            assert_eq!(reqs[0].server_id, "a3f901c48b2e7d10");
            assert_eq!(reqs[0].farm_id, Some(2));
            assert_eq!(reqs[0].farm_name.as_deref(), Some("Südhof & Co"));
            assert_eq!(reqs[0].fill_type.as_deref(), Some("BARLEY"));
            assert_eq!(reqs[0].liters, Some(25_000));
            assert_eq!(reqs[0].price_delta_pct, Some(-7));
            assert_eq!(reqs[0].allow_partial, Some(false));
            assert_eq!(reqs[0].duration_hours, Some(48));
            assert_eq!(reqs[0].was_bale, Some(true));
            assert_eq!(reqs[0].was_pallet, Some(false));

            assert_eq!(reqs[1].sub_action, ContractSubAction::AckSettlement);
            assert_eq!(reqs[1].settlement_ids.as_deref(), Some("set_901,set_902"));
        }
        other => panic!("unexpected packet: {other:?}"),
    }

    // 3. Incoming gmContractSync_*.xml writer & isolation from gmDataSync_*.xml pruning
    let contract_state = ContractStateResponse {
        server_secret: None,
        updated_at: 1_763_395_200,
        contracts: vec![OnlineContractDto {
            remote_id: "c_101".to_string(),
            origin: "ONLINE".to_string(),
            is_npc: false,
            issuer_server_id: "99aa11bb22cc33dd".to_string(),
            issuer_farm_id: 1,
            issuer_name: "Nordhof <Server A>".to_string(),
            contractor_server_id: String::new(),
            contractor_farm_id: 0,
            fill_type: "WHEAT".to_string(),
            total_liters: 5_000_000_000, // Clamped to i32::MAX
            delivered_liters: 0,
            base_trend_factor: 1.05,
            bonus_pct: 12,
            remaining_seconds: 172_800,
            status: "OPEN".to_string(),
        }],
        direct_offers: vec![OnlineDirectOfferDto {
            remote_id: "o_205".to_string(),
            origin: "ONLINE".to_string(),
            is_npc: false,
            seller_server_id: "99aa11bb22cc33dd".to_string(),
            seller_farm_id: 2,
            seller_name: "Südhof".to_string(),
            fill_type: "BARLEY".to_string(),
            total_liters: 25_000,
            remaining_liters: 25_000,
            price_delta_pct: -5,
            allow_partial: true,
            remaining_seconds: 86_400,
            was_bale: false,
            was_pallet: false,
            status: "OPEN".to_string(),
        }],
        action_results: vec![ContractActionResultDto {
            action_id: "a3f901c48b2e7d10_12000_1".to_string(),
            sub_action: "BUY_OFFER".to_string(),
            status: "ACCEPTED".to_string(),
            reason: String::new(),
            remote_id: "o_205".to_string(),
            granted_liters: 10_000,
            base_trend_factor: 1.0,
            bonus_pct: 0,
            remaining_seconds: 0,
        }],
        settlements: vec![ContractSettlementDto {
            settlement_id: "set_901".to_string(),
            settlement_type: "CONTRACT_DELIVERY".to_string(),
            target_server_id: "a3f901c48b2e7d10".to_string(),
            target_farm_id: 1,
            remote_id: "c_101".to_string(),
            fill_type: "WHEAT".to_string(),
            liters: 20_000,
            missing_liters: 0,
            base_trend_factor: 1.0,
            price_delta_pct: 0,
            source_label: "#c_101 (Südhof)".to_string(),
        }],
    };

    let contract_xml_path =
        write_contract_data_xml(&staging_dir, &read_dir, &contract_state).unwrap();
    let fname = contract_xml_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap();
    assert!(
        fname.starts_with("gmContractSync_"),
        "Contract sync packet MUST use gmContractSync_ prefix, got {fname}"
    );
    let raw_bytes = fs::read(&contract_xml_path).unwrap();
    assert!(!raw_bytes.starts_with(&[0xEF, 0xBB, 0xBF]));
    let xml_str = String::from_utf8(raw_bytes).unwrap();
    assert!(xml_str.contains("<gmDataSync action=\"contractData\">"));
    assert!(xml_str.contains("totalLiters=\"2147483647\""));
    assert!(xml_str.contains("baseTrendFactor=\"1.0500\""));
    assert!(xml_str.contains("issuerName=\"Nordhof &lt;Server A&gt;\""));
    assert!(xml_str.contains("priceDeltaPct=\"-5\""));
    assert!(xml_str.contains("actionId=\"a3f901c48b2e7d10_12000_1\""));
    assert!(xml_str.contains("settlementId=\"set_901\" type=\"CONTRACT_DELIVERY\""));

    // Writing 10 spot-market snapshots MUST NOT evict unread gmContractSync_*.xml!
    for idx in 1..=10 {
        write_market_data_xml(
            &staging_dir,
            &read_dir,
            &[MarketItemDto {
                fill_type: "WHEAT".to_string(),
                title: None,
                fill_level: 500_000 + idx * 1_000,
                trend: 1.0,
                great_demand: false,
                great_demand_bonus: 0.0,
                great_demand_remaining: 0,
                status: FillTypeStatus::Approved,
            }],
        )
        .unwrap();
    }

    assert!(
        contract_xml_path.exists(),
        "Spot-market gmDataSync pruning must NEVER delete unread gmContractSync_*.xml!"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stage2_end_to_end_contracts_offers_settlements_and_fastapi_http() {
    // Part A: FastAPI HTTP client ETag scoping per server_id & POST /api/v1/contracts/actions
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback server");
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    let server_thread = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        for step in 0..5 {
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("accept: {e}"),
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
            let get_hdr = |name: &str| -> Option<String> {
                headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            };

            let response = match step {
                0 => {
                    assert!(request_line.starts_with("GET /api/v1/meta/status "));
                    let json = r#"{"online":true,"min_exe_version":"0.9.10","poll_interval_sec":10,"market_name":"Global Market","api_version":"2.1.0","contracts_supported":true}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                1 => {
                    assert!(request_line.starts_with("GET /api/v1/contracts/state "));
                    let json = r#"{"updated_at":1763395200,"contracts":[{"remote_id":"c_1","origin":"ONLINE","is_npc":false,"issuer_server_id":"srv_other","issuer_farm_id":1,"issuer_name":"Nordhof","contractor_server_id":"","contractor_farm_id":0,"fill_type":"WHEAT","total_liters":50000,"delivered_liters":0,"base_trend_factor":1.0,"bonus_pct":12,"remaining_seconds":86400,"status":"OPEN"}],"direct_offers":[],"action_results":[],"settlements":[]}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"cetag-anon\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                2 => {
                    // Switching from server_id=None to server_id=Some("srv_a") MUST NOT reuse "cetag-anon"!
                    assert!(
                        request_line.starts_with("GET /api/v1/contracts/state?server_id=srv_a ")
                    );
                    assert_eq!(get_hdr("if-none-match"), None);
                    let json = r#"{"updated_at":1763395205,"contracts":[{"remote_id":"c_1","origin":"ONLINE","is_npc":false,"issuer_server_id":"srv_other","issuer_farm_id":1,"issuer_name":"Nordhof","contractor_server_id":"srv_a","contractor_farm_id":1,"fill_type":"WHEAT","total_liters":50000,"delivered_liters":10000,"base_trend_factor":1.0,"bonus_pct":12,"remaining_seconds":86395,"status":"ACTIVE"}],"direct_offers":[],"action_results":[],"settlements":[]}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"cetag-srva\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        json.len(),
                        json
                    )
                }
                3 => {
                    // Repeat poll with same server_id="srv_a" sends If-None-Match: "cetag-srva" -> 304 Not Modified
                    assert!(
                        request_line.starts_with("GET /api/v1/contracts/state?server_id=srv_a ")
                    );
                    assert_eq!(get_hdr("if-none-match").as_deref(), Some("\"cetag-srva\""));
                    "HTTP/1.1 304 Not Modified\r\nETag: \"cetag-srva\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                }
                4 => {
                    assert!(request_line.starts_with("POST /api/v1/contracts/actions "));
                    assert!(body_str.contains("\"sub_action\":\"CREATE_OFFER\""));
                    assert!(body_str.contains("\"action_id\":\"srv_a_9000_1\""));
                    assert!(body_str.contains("\"price_delta_pct\":-5"));
                    let json = r#"{"updated_at":1763395210,"contracts":[],"direct_offers":[{"remote_id":"o_99","origin":"ONLINE","is_npc":false,"seller_server_id":"srv_a","seller_farm_id":1,"seller_name":"Hof A","fill_type":"BARLEY","total_liters":20000,"remaining_liters":20000,"price_delta_pct":-5,"allow_partial":true,"remaining_seconds":86400,"was_bale":false,"was_pallet":false,"status":"OPEN"}],"action_results":[{"action_id":"srv_a_9000_1","sub_action":"CREATE_OFFER","status":"ACCEPTED","reason":"","remote_id":"o_99","granted_liters":0,"base_trend_factor":1.0,"bonus_pct":0,"remaining_seconds":86400}],"settlements":[]}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"cetag-srva-2\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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

    let api = FastApiBackend::new(format!("http://{addr}"), None, "cid1234567890abc".to_string())
        .unwrap();
    let hs = api.check_status().unwrap();
    assert!(hs.contracts_supported);

    let srv_secret = secret("srv-a-secret");
    let s_anon = api.fetch_contracts_state(None, None).unwrap();
    assert_eq!(s_anon.contracts.len(), 1);
    assert_eq!(s_anon.contracts[0].status, "OPEN");

    let s_srva = api.fetch_contracts_state(Some("srv_a"), Some(&srv_secret)).unwrap();
    assert_eq!(s_srva.contracts[0].status, "ACTIVE");

    let s_cached = api.fetch_contracts_state(Some("srv_a"), Some(&srv_secret)).unwrap();
    assert_eq!(s_cached.contracts[0].delivered_liters, 10_000);

    let after_post = api
        .submit_contract_action(ContractActionRequest {
            sub_action: ContractSubAction::CreateOffer,
            action_id: "srv_a_9000_1".to_string(),
            server_id: "srv_a".to_string(),
            farm_id: Some(1),
            farm_name: Some("Hof A".to_string()),
            remote_id: None,
            fill_type: Some("BARLEY".to_string()),
            liters: Some(20_000),
            base_trend_factor: None,
            bonus_pct: None,
            price_delta_pct: Some(-5),
            allow_partial: Some(true),
            duration_hours: Some(24),
            was_bale: Some(false),
            was_pallet: Some(false),
            settlement_ids: None,
            server_secret: None,
        }, Some(&srv_secret))
        .unwrap();
    assert_eq!(after_post.direct_offers.len(), 1);
    assert_eq!(after_post.action_results[0].remote_id, "o_99");
    server_thread.join().unwrap();

    // Part B: Full Coordinator + LocalMockBackend E2E lifecycle (CREATE -> SIMULATE SETTLEMENT -> ACK_SETTLEMENT)
    let test_dir = env::temp_dir().join(format!(
        "fs25_gm_e2e_contracts_mgr_{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&test_dir);
    let profile_dir = test_dir.join("FarmingSimulator2025");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(profile_dir.join("gameSettings.xml"), "<gameSettings/>").unwrap();

    let config_file = test_dir.join("config").join("settings.json");
    AppConfig {
        access_tier: AccessTier::Developer,
        backend_mode: BackendMode::Mock,
        fs25_profile_path: Some(profile_dir.to_string_lossy().into_owned()),
        ..AppConfig::default()
    }
    .save_to_file(&config_file)
    .unwrap();

    let mgr = BridgeManager::new(config_file, None);
    let bridge_root = profile_dir.join("modSettings").join("FS25_GlobalMarket");
    let write_dir = bridge_root.join("write");
    let read_dir = bridge_root.join("read");
    let online_file = bridge_root.join("globalMarketOnline.xml");

    for _ in 0..40 {
        if online_file
            .exists()
            .then(|| fs::read_to_string(&online_file).ok())
            .flatten()
            .is_some_and(|s| s.contains("contractsSupported=\"true\""))
        {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let hb_xml = fs::read_to_string(&online_file).unwrap();
    assert!(hb_xml.contains("contractsSupported=\"true\""));

    // Clear initial read_dir files as Lua would, then emit _2 (CREATE_CONTRACT) and _10 (CREATE_OFFER)
    for entry in fs::read_dir(&read_dir).unwrap().flatten() {
        let _ = fs::remove_file(entry.path());
    }

    fs::write(
        write_dir.join("contractAction_20261117_200500_2.xml"),
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="contractAction">
    <request subAction="CREATE_CONTRACT" actionId="a3f901c48b2e7d10_1000_2" serverId="a3f901c48b2e7d10" farmId="1" farmName="Hof Meier [Dedi]" fillType="WHEAT" liters="30000" baseTrendFactor="1.0" bonusPct="15" durationHours="48" />
</gmDataSync>"#,
    )
    .unwrap();
    fs::write(
        write_dir.join("contractAction_20261117_200500_10.xml"),
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="contractAction">
    <request subAction="CREATE_OFFER" actionId="a3f901c48b2e7d10_1000_10" serverId="a3f901c48b2e7d10" farmId="1" farmName="Hof Meier [Dedi]" fillType="BARLEY" liters="15000" priceDeltaPct="-4" allowPartial="true" durationHours="24" />
</gmDataSync>"#,
    )
    .unwrap();

    for _ in 0..80 {
        let write_left = fs::read_dir(&write_dir).unwrap().flatten().count();
        let snap = mgr.get_snapshot();
        if write_left == 0 && snap.open_contracts_count >= 2 && snap.open_offers_count >= 2 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let snap = mgr.get_snapshot();
    assert!(snap.contracts_supported);
    assert!(snap.open_contracts_count >= 2);
    assert!(snap.open_offers_count >= 2);

    let contract_sync_xmls: String = fs::read_dir(&read_dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("gmContractSync_"))
        })
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .collect();
    assert!(contract_sync_xmls.contains("actionId=\"a3f901c48b2e7d10_1000_2\" subAction=\"CREATE_CONTRACT\" status=\"ACCEPTED\""));
    assert!(contract_sync_xmls.contains("actionId=\"a3f901c48b2e7d10_1000_10\" subAction=\"CREATE_OFFER\" status=\"ACCEPTED\""));

    // Clear read_dir, trigger Mock foreign counterparty fulfillment, and verify settlements arrive in gmContractSync_*.xml
    for entry in fs::read_dir(&read_dir).unwrap().flatten() {
        let _ = fs::remove_file(entry.path());
    }
    let sim_summary = mgr.mock_simulate_contract_activity().unwrap();
    assert!(sim_summary.contains("fulfilled=1, sold=1"));

    let mut settlement_xml = String::new();
    for _ in 0..60 {
        settlement_xml = fs::read_dir(&read_dir)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("gmContractSync_"))
            })
            .filter_map(|e| fs::read_to_string(e.path()).ok())
            .collect();
        if settlement_xml.contains("type=\"CONTRACT_DELIVERY\"")
            && settlement_xml.contains("type=\"OFFER_SOLD\"")
        {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(settlement_xml.contains("type=\"CONTRACT_DELIVERY\""));
    assert!(settlement_xml.contains("type=\"CONTRACT_COMPLETED_ISSUER\""));
    assert!(settlement_xml.contains("type=\"OFFER_SOLD\""));

    // Acknowledge settlements via ACK_SETTLEMENT
    for entry in fs::read_dir(&read_dir).unwrap().flatten() {
        let _ = fs::remove_file(entry.path());
    }
    fs::write(
        write_dir.join("contractAction_20261117_200600_11.xml"),
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="contractAction">
    <request subAction="ACK_SETTLEMENT" actionId="a3f901c48b2e7d10_2000_11" serverId="a3f901c48b2e7d10" settlementIds="mock_set_1,mock_set_2,mock_set_3" />
</gmDataSync>"#,
    )
    .unwrap();

    for _ in 0..60 {
        if fs::read_dir(&write_dir).unwrap().flatten().count() == 0 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let post_ack_xml: String = fs::read_dir(&read_dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("gmContractSync_"))
        })
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .collect();
    assert!(!post_ack_xml.contains("settlementId=\"mock_set_1\""));

    mgr.shutdown();
    assert!(!online_file.exists());
    assert_eq!(fs::read_dir(&read_dir).unwrap().flatten().count(), 0);
    let _ = fs::remove_dir_all(&test_dir);
}

#[test]
fn all_16_locale_files_have_100_percent_key_and_placeholder_parity() {
    use std::collections::{BTreeMap, BTreeSet};

    fn extract_placeholders(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '{' {
                let mut token = String::new();
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                    token.push(inner);
                }
                if !token.is_empty() {
                    out.insert(token);
                }
            }
        }
        out
    }

    let locales_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("src")
        .join("locales");

    let expected_langs = [
        "de", "en", "fr", "es", "pl", "it", "nl", "cz", "pt", "hu", "da", "sv", "ro", "tr", "ru",
        "uk",
    ];

    let en_raw = fs::read_to_string(locales_dir.join("en.json")).expect("read en.json");
    let en_map: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(&en_raw).expect("valid JSON in en.json");
    let en_keys: BTreeSet<String> = en_map.keys().cloned().collect();

    for lang in expected_langs {
        let path = locales_dir.join(format!("{lang}.json"));
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read locale {lang}.json: {e}"));
        let map: BTreeMap<String, serde_json::Value> = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("Invalid JSON in {lang}.json: {e}"));
        let keys: BTreeSet<String> = map.keys().cloned().collect();

        let missing: Vec<_> = en_keys.difference(&keys).collect();
        let extra: Vec<_> = keys.difference(&en_keys).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "Locale {lang}.json key mismatch vs en.json! missing={missing:?}, extra={extra:?}"
        );

        for (k, en_val) in &en_map {
            if let (Some(en_str), Some(loc_str)) =
                (en_val.as_str(), map.get(k).and_then(|v| v.as_str()))
            {
                assert_eq!(
                    extract_placeholders(en_str),
                    extract_placeholders(loc_str),
                    "Placeholder mismatch in {lang}.json for key '{k}'"
                );
            }
        }
    }
}

// ---- Server notices & maintenance (Spec Section 3.1.3) ----

fn test_notice(notice_type: &str, starts_at: Option<u64>, ends_at: Option<u64>) -> ServerNotice {
    ServerNotice {
        id: "n-1".to_string(),
        notice_type: notice_type.to_string(),
        starts_at,
        ends_at,
        message: Default::default(),
    }
}

#[test]
fn server_notice_wire_parsing_is_lenient_and_windows_are_correct() {
    let parsed = ServerNotice::from_wire(&serde_json::json!({
        "id": "  2026-12-01-maintenance ",
        "type": "MAINTENANCE",
        "starts_at": 1_764_621_000u64,
        "ends_at": 1_764_624_600u64,
        "message": { "DE": "  Datenbank-Umzug ", "en": "Database migration", "fr": "" }
    }))
    .expect("valid notice");
    assert_eq!(parsed.id, "2026-12-01-maintenance");
    assert_eq!(parsed.notice_type, "maintenance");
    assert_eq!(parsed.starts_at, Some(1_764_621_000));
    assert_eq!(parsed.ends_at, Some(1_764_624_600));
    assert_eq!(parsed.message.get("de").map(String::as_str), Some("Datenbank-Umzug"));
    assert!(!parsed.message.contains_key("fr"), "empty texts are dropped");

    // Unknown types and downtime notices without a start degrade to `info`.
    let unknown = ServerNotice::from_wire(&serde_json::json!({ "id": "x", "type": "party" })).unwrap();
    assert_eq!(unknown.notice_type, "info");
    let no_start =
        ServerNotice::from_wire(&serde_json::json!({ "id": "x", "type": "restart" })).unwrap();
    assert_eq!(no_start.notice_type, "info");
    // An end before the start is ignored.
    let bad_end = ServerNotice::from_wire(&serde_json::json!({
        "id": "x", "type": "maintenance", "starts_at": 2000, "ends_at": 1000
    }))
    .unwrap();
    assert_eq!(bad_end.ends_at, None);
    // Missing id or a non-object notice is rejected.
    assert!(ServerNotice::from_wire(&serde_json::json!({ "type": "info" })).is_none());
    assert!(ServerNotice::from_wire(&serde_json::json!({ "id": "   " })).is_none());
    assert!(ServerNotice::from_wire(&serde_json::json!("maintenance")).is_none());
    let long_id = ServerNotice::from_wire(&serde_json::json!({ "id": "a".repeat(200) })).unwrap();
    assert_eq!(long_id.id.len(), 64);

    // Restart without end lasts 600 s; outages count from 300 s before the start until 1800 s after the end.
    let restart = test_notice("restart", Some(10_000), None);
    assert_eq!(restart.display_end(), Some(10_600));
    assert!(!restart.covers_outage_at(9_699));
    assert!(restart.covers_outage_at(9_700));
    assert!(restart.covers_outage_at(12_400));
    assert!(!restart.covers_outage_at(12_401));
    assert!(restart.is_expired(12_401));

    // Open-ended maintenance shows no end but still expires after the 6 h fallback.
    let open = test_notice("maintenance", Some(10_000), None);
    assert_eq!(open.display_end(), None);
    assert!(open.covers_outage_at(10_000 + 6 * 3600));
    assert!(!open.covers_outage_at(10_000 + 6 * 3600 + 1801));

    // Info notices never cover an outage and only expire with an explicit end.
    let info = test_notice("info", None, None);
    assert!(!info.covers_outage_at(10_000));
    assert!(!info.is_expired(u64::MAX / 2));

    let mut texts = test_notice("info", None, None);
    texts.message.insert("cs".to_string(), "Údržba".to_string());
    texts.message.insert("en".to_string(), "Maintenance".to_string());
    assert_eq!(texts.message_for(&["cs"]).as_deref(), Some("Údržba"));
    assert_eq!(texts.message_for(&["de"]).as_deref(), Some("Maintenance"));
    assert_eq!(
        message_languages(Some("CZ"), "de"),
        vec!["cs".to_string(), "cz".to_string(), "de".to_string()]
    );
    assert_eq!(message_languages(None, "en"), vec!["en".to_string()]);

    assert_eq!(format_unix_utc(0), "1970-01-01 00:00 UTC");
    assert_eq!(format_unix_utc(951_782_400), "2000-02-29 00:00 UTC");
    assert_eq!(format_unix_utc(1_764_621_000), "2025-12-01 20:30 UTC");
    assert_eq!(format_unix_utc(4_102_444_799), "2099-12-31 23:59 UTC");
}

#[test]
fn maintenance_errors_are_classified_and_merged_into_notices() {
    let detail = serde_json::json!({
        "detail": { "code": "MAINTENANCE", "notice_id": "n-1", "until": 20_000 }
    });
    match parse_maintenance_error(&detail, Some(60)) {
        Some(ApiError::Maintenance { notice_id, until }) => {
            assert_eq!(notice_id.as_deref(), Some("n-1"));
            assert_eq!(until, Some(20_000), "explicit `until` wins over Retry-After");
        }
        other => panic!("expected Maintenance, got {other:?}"),
    }
    match parse_maintenance_error(&serde_json::json!({ "detail": "maintenance" }), Some(120)) {
        Some(ApiError::Maintenance {
            notice_id: None,
            until: Some(until),
        }) => {
            assert!(until > 1_700_000_000, "Retry-After is converted to an absolute time");
        }
        other => panic!("expected Maintenance from Retry-After, got {other:?}"),
    }
    assert!(
        parse_maintenance_error(&serde_json::json!({ "detail": { "code": "OTHER" } }), None)
            .is_none()
    );

    let window = test_notice("maintenance", Some(10_000), Some(13_600));
    let maintenance = ApiError::Maintenance {
        notice_id: None,
        until: None,
    };
    let network = ApiError::Network("connection refused".to_string());
    assert_eq!(classify_outage(&maintenance, None, 1), Some(OfflineReason::Maintenance));
    assert_eq!(
        classify_outage(&network, Some(&window), 11_000),
        Some(OfflineReason::Maintenance)
    );
    assert_eq!(
        classify_outage(&network, Some(&window), 5_000),
        Some(OfflineReason::Unreachable)
    );
    assert_eq!(classify_outage(&network, None, 11_000), Some(OfflineReason::Unreachable));
    assert_eq!(classify_outage(&ApiError::Unauthorized, Some(&window), 11_000), None);
    assert_eq!(OfflineReason::Maintenance.as_str(), "maintenance");

    // A matching announced notice is kept and gets the newer end.
    let merged = notice_for_maintenance_error(Some(&window), Some("n-1"), Some(15_000), 11_000);
    assert_eq!(
        (merged.id.as_str(), merged.starts_at, merged.ends_at),
        ("n-1", Some(10_000), Some(15_000))
    );
    // Without `until` the announced end stays.
    let kept = notice_for_maintenance_error(Some(&window), None, None, 11_000);
    assert_eq!(kept.ends_at, Some(13_600));
    // Another id or no known notice creates a maintenance notice that starts now.
    let other = notice_for_maintenance_error(Some(&window), Some("n-2"), Some(15_000), 11_000);
    assert_eq!(
        (other.id.as_str(), other.starts_at, other.ends_at),
        ("n-2", Some(11_000), Some(15_000))
    );
    let synthetic = notice_for_maintenance_error(None, None, Some(15_000), 11_000);
    assert_eq!(synthetic.id, "maintenance-15000");
    assert_eq!(synthetic.notice_type, "maintenance");
    // An `until` in the past is ignored.
    let past = notice_for_maintenance_error(None, None, Some(100), 11_000);
    assert_eq!(past.ends_at, None);
}

#[test]
fn heartbeat_xml_writes_server_notice_and_offline_reason() {
    let dir = env::temp_dir().join(format!("gm_hb_notice_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let staging = dir.join(".staging");
    let online = dir.join("globalMarketOnline.xml");
    fs::create_dir_all(&staging).unwrap();

    let long_text = format!("Wartung <DB> & \"Umzug\" {}", "ä".repeat(100));
    let notice = HeartbeatNotice {
        id: "2026-12-01-maintenance".to_string(),
        notice_type: "maintenance".to_string(),
        starts_at: Some(1_764_621_000),
        ends_at: Some(1_764_624_600),
        message: Some(long_text),
    };
    let write = |connected: bool, notice: Option<&HeartbeatNotice>, reason: Option<&str>| {
        write_heartbeat_xml_with_extras(
            &staging,
            &online,
            connected,
            "0.9.16",
            "0.9.10",
            None,
            None,
            false,
            HeartbeatExtras {
                market_profiles: None,
                server_notice: notice,
                offline_reason: reason,
                vehicles_supported: false,
            },
        )
        .unwrap();
        fs::read_to_string(&online).unwrap()
    };

    let online_xml = write(true, Some(&notice), Some("maintenance"));
    assert!(
        !heartbeat_root(&online_xml).contains("offlineReason"),
        "no reason while connected"
    );
    let line = online_xml
        .lines()
        .find(|l| l.contains("<serverNotice"))
        .expect("serverNotice line");
    assert!(line.starts_with(
        "    <serverNotice id=\"2026-12-01-maintenance\" type=\"maintenance\" startsAt=\"1764621000\" endsAt=\"1764624600\" message=\"Wartung &lt;DB&gt; &amp; &quot;Umzug&quot; "
    ));
    assert!(line.ends_with(" />"));
    let raw_message = line
        .split("message=\"")
        .nth(1)
        .unwrap()
        .trim_end_matches("\" />");
    let unescaped = raw_message
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");
    assert!(
        unescaped.len() <= MAX_NOTICE_MESSAGE_BYTES,
        "message is capped at 120 UTF-8 bytes"
    );
    assert!(unescaped.ends_with('ä'), "truncation keeps whole UTF-8 characters");
    assert!(online_xml.find("<serverNotice").unwrap() > online_xml.find("<minVersion>").unwrap());

    let offline_xml = write(false, Some(&notice), Some("maintenance"));
    let root = heartbeat_root(&offline_xml);
    assert!(root.contains("isConnected=\"false\""));
    assert!(root.contains("offlineReason=\"maintenance\""));
    assert!(
        offline_xml.contains("<serverNotice id=\"2026-12-01-maintenance\""),
        "notice stays while offline"
    );

    let unreachable_xml = write(false, None, Some("unreachable"));
    assert!(heartbeat_root(&unreachable_xml).contains("offlineReason=\"unreachable\""));
    assert!(!unreachable_xml.contains("<serverNotice"));

    let unknown_reason_xml = write(false, None, Some("banana"));
    assert!(!unknown_reason_xml.contains("offlineReason"));

    let info = HeartbeatNotice {
        id: "info-1".to_string(),
        notice_type: "something-new".to_string(),
        starts_at: None,
        ends_at: None,
        message: None,
    };
    let info_xml = write(true, Some(&info), None);
    assert!(info_xml.contains("    <serverNotice id=\"info-1\" type=\"info\" />\n"));

    let no_id = HeartbeatNotice {
        id: "  ".to_string(),
        ..info
    };
    assert!(!write(true, Some(&no_id), None).contains("<serverNotice"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stored_server_notice_persists_and_drops_expired_notices() {
    let dir = env::temp_dir().join(format!("gm_notice_store_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = StoredServerNotice::path_for_config(&dir.join("settings.json"));
    assert_eq!(path.file_name().unwrap(), "server_notice.json");

    assert_eq!(StoredServerNotice::load(&path, 0), StoredServerNotice::default());

    let stored = StoredServerNotice {
        notice: Some(test_notice("maintenance", Some(10_000), Some(13_600))),
        notified_id: Some("n-1".to_string()),
    };
    stored.save(&path).unwrap();
    assert_eq!(StoredServerNotice::load(&path, 12_000), stored);

    let expired = StoredServerNotice::load(&path, 13_600 + 1801);
    assert_eq!(expired.notice, None, "expired notices are dropped on load");
    assert_eq!(expired.notified_id.as_deref(), Some("n-1"), "the announced id is kept");

    fs::write(&path, "not json").unwrap();
    assert_eq!(StoredServerNotice::load(&path, 0), StoredServerNotice::default());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fastapi_parses_status_notice_and_503_maintenance() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server_thread = thread::spawn(move || {
        let responses = [
            (
                "200 OK",
                r#"{"online":true,"min_exe_version":"0.9.9","market_name":"Global Market","notice":{"id":"n-7","type":"restart","starts_at":1764621000,"message":{"en":"Kernel update"}}}"#,
                "",
            ),
            (
                "200 OK",
                r#"{"online":true,"min_exe_version":"0.9.9","market_name":"Global Market","notice":{"type":"broken"}}"#,
                "",
            ),
            (
                "503 Service Unavailable",
                r#"{"detail":{"code":"MAINTENANCE","notice_id":"n-7","until":1764624600}}"#,
                "Retry-After: 600\r\n",
            ),
            ("503 Service Unavailable", "<html>Bad Gateway</html>", ""),
        ];
        for (status, body, extra_headers) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&mut stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            drop(reader);
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });

    let backend = FastApiBackend::new(
        format!("http://{addr}"),
        None,
        "a1b2c3d4e5f60718".to_string(),
    )
    .expect("client");

    let hs = backend.check_status().expect("status with notice");
    let notice = hs.notice.expect("notice parsed");
    assert_eq!(notice.id, "n-7");
    assert_eq!(notice.notice_type, "restart");
    assert_eq!(notice.display_end(), Some(1_764_621_600));
    assert_eq!(notice.message_for(&[]).as_deref(), Some("Kernel update"));

    let hs_broken = backend
        .check_status()
        .expect("malformed notice must not fail the handshake");
    assert!(hs_broken.notice.is_none());

    match backend.check_status() {
        Err(ApiError::Maintenance { notice_id, until }) => {
            assert_eq!(notice_id.as_deref(), Some("n-7"));
            assert_eq!(until, Some(1_764_624_600));
        }
        other => panic!("expected Maintenance, got {other:?}"),
    }

    match backend.check_status() {
        Err(ApiError::Network(msg)) => assert!(msg.contains("503")),
        other => panic!("plain 503 must stay a network error, got {other:?}"),
    }

    server_thread.join().unwrap();
}

#[test]
fn mock_maintenance_flow_updates_heartbeat_snapshot_and_logs_end_to_end() {
    let test_dir = env::temp_dir().join(format!("fs25_gm_notice_e2e_{}", std::process::id()));
    let _ = fs::remove_dir_all(&test_dir);
    let profile_dir = test_dir.join("FarmingSimulator2025");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(profile_dir.join("gameSettings.xml"), "<gameSettings/>").unwrap();

    let config_file = test_dir.join("config").join("settings.json");
    AppConfig {
        access_tier: AccessTier::Developer,
        backend_mode: BackendMode::Mock,
        language: "en".to_string(),
        fs25_profile_path: Some(profile_dir.to_string_lossy().into_owned()),
        ..AppConfig::default()
    }
    .save_to_file(&config_file)
    .unwrap();

    let manager = BridgeManager::new(config_file.clone(), None);
    let online_file = profile_dir
        .join("modSettings")
        .join("FS25_GlobalMarket")
        .join("globalMarketOnline.xml");
    let wait_for = |what: &str, pred: &dyn Fn(&str) -> bool| -> String {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let xml = fs::read_to_string(&online_file).unwrap_or_default();
            if pred(&xml) {
                return xml;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; last heartbeat:\n{xml}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    };

    wait_for("online heartbeat", &|xml| xml.contains("isConnected=\"true\""));

    // 1. Maintenance announced: notice in heartbeat and snapshot, still online.
    manager.mock_set_server_notice_stage(1);
    let announced = wait_for("serverNotice", &|xml| xml.contains("<serverNotice"));
    assert!(heartbeat_root(&announced).contains("isConnected=\"true\""));
    assert!(announced.contains("type=\"maintenance\""));
    assert!(announced.contains("message=\"Simulated maintenance (Mock)\""));
    let snap = manager.get_snapshot();
    let notice_id = snap.server_notice.as_ref().expect("snapshot notice").id.clone();
    assert_eq!(snap.offline_reason, None);
    let stored = StoredServerNotice::load(&StoredServerNotice::path_for_config(&config_file), 0);
    assert_eq!(stored.notified_id.as_deref(), Some(notice_id.as_str()));

    // 2. Maintenance running: offline with reason, notice kept.
    manager.mock_set_server_notice_stage(2);
    let running = wait_for("offlineReason", &|xml| {
        xml.contains("offlineReason=\"maintenance\"")
    });
    assert!(heartbeat_root(&running).contains("isConnected=\"false\""));
    assert!(running.contains(&format!("<serverNotice id=\"{notice_id}\"")));
    let snap = manager.get_snapshot();
    assert_eq!(snap.state, BridgeConnectionState::BackendOffline);
    assert_eq!(snap.offline_reason.as_deref(), Some("maintenance"));

    // 3. Maintenance over: back online, notice and reason gone.
    manager.mock_set_server_notice_stage(0);
    let restored = wait_for("reconnect", &|xml| {
        xml.contains("isConnected=\"true\"") && !xml.contains("<serverNotice")
    });
    assert!(!restored.contains("offlineReason"));
    let snap = manager.get_snapshot();
    assert_eq!(snap.server_notice, None);
    assert_eq!(snap.offline_reason, None);

    let logs = manager.get_logs();
    let count = |key: &str| logs.iter().filter(|l| l.key.as_deref() == Some(key)).count();
    assert_eq!(count("logServerNoticeMaintenance"), 1, "a notice is announced once per id");
    assert_eq!(count("logServerMaintenanceActive"), 1, "maintenance start is logged once");

    manager.shutdown();
    let _ = fs::remove_dir_all(&test_dir);
}

// ---------------------------------------------------------------------------
// Savegame identity (`X-Server-Secret`, Spec Section 3.5)
// ---------------------------------------------------------------------------

const IDENTITY_SERVER_ID: &str = "a3f901c48b2e7d10";

fn secret(raw: &str) -> ServerSecret {
    ServerSecret::parse(raw).expect("valid secret")
}

fn sync_request(action_id: &str, server_secret: Option<ServerSecret>) -> ContractActionRequest {
    ContractActionRequest {
        sub_action: ContractSubAction::RequestSync,
        action_id: action_id.to_string(),
        server_id: IDENTITY_SERVER_ID.to_string(),
        farm_id: None,
        farm_name: None,
        remote_id: None,
        fill_type: None,
        liters: None,
        base_trend_factor: None,
        bonus_pct: None,
        price_delta_pct: None,
        allow_partial: None,
        duration_hours: None,
        was_bale: None,
        was_pallet: None,
        settlement_ids: None,
        server_secret,
    }
}

fn read_contract_sync_xmls(read_dir: &std::path::Path) -> Vec<String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(read_dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("gmContractSync_"))
    });
    paths.sort();
    paths
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect()
}

fn clear_dir(dir: &std::path::Path) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let _ = fs::remove_file(entry.path());
    }
}

/// Writes an action file like the mod and waits until the client answered it.
fn send_action_xml(
    bridge_root: &std::path::Path,
    seq: u32,
    attributes: &str,
    answered: &dyn Fn(&str) -> bool,
) -> String {
    let write_dir = bridge_root.join("write");
    let read_dir = bridge_root.join("read");
    let tmp = write_dir.join(format!("contractAction_20261124_120000_{seq}.tmp"));
    fs::write(
        &tmp,
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"no\"?>\n<gmDataSync action=\"contractAction\">\n    <request serverId=\"{IDENTITY_SERVER_ID}\" {attributes} />\n</gmDataSync>"
        ),
    )
    .unwrap();
    fs::rename(&tmp, tmp.with_extension("xml")).unwrap();
    for _ in 0..200 {
        let joined = read_contract_sync_xmls(&read_dir).join("\n");
        if fs::read_dir(&write_dir).unwrap().flatten().count() == 0 && answered(&joined) {
            return joined;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "action {seq} was not answered, read/: {:?}",
        read_contract_sync_xmls(&read_dir)
    );
}

fn extract_attr(xml: &str, element: &str, attr: &str) -> Option<String> {
    let start = xml.find(&format!("<{element} "))?;
    let rest = &xml[start..];
    let rest = &rest[..rest.find("/>")?];
    let from = rest.find(&format!("{attr}=\""))? + attr.len() + 2;
    let value = &rest[from..];
    Some(value[..value.find('"')?].to_string())
}

#[test]
fn server_secret_is_validated_redacted_and_never_serialized() {
    assert!(ServerSecret::parse("  abc-DEF_123.~+/=  ").is_some());
    for bad in ["", "   ", "a b", "a\"b", "<x>", "a&b", "ä", "line\nbreak"] {
        assert!(ServerSecret::parse(bad).is_none(), "{bad:?} must be refused");
    }
    assert!(ServerSecret::parse(&"x".repeat(256)).is_some());
    assert!(ServerSecret::parse(&"x".repeat(257)).is_none());

    let req = sync_request("a1", Some(secret("top-secret-value")));
    assert!(!format!("{req:?}").contains("top-secret-value"));
    assert!(!serde_json::to_string(&req).unwrap().contains("top-secret-value"));

    let state = ContractStateResponse {
        server_secret: Some(secret("top-secret-value")),
        ..ContractStateResponse::default()
    };
    assert!(!format!("{state:?}").contains("top-secret-value"));
    assert!(!serde_json::to_string(&state).unwrap().contains("top-secret-value"));
}

#[test]
fn server_identity_store_picks_secrets_and_tracks_delivery() {
    let (live, sid) = ("https://gm.example", IDENTITY_SERVER_ID);
    let (a, b) = (secret("secret-a"), secret("secret-b"));
    let mut store = ServerIdentityStore::default();

    // Nothing known: one attempt without a secret, board-only poll.
    assert_eq!(store.candidates_for_action(live, sid, None), vec![None]);
    assert_eq!(store.secret_for_poll(live, sid), None);
    assert_eq!(store.state(live, sid), ServerIdentityState::Unknown);

    // Registration: remembered, persisted and still to be delivered to the mod.
    assert!(store.record_accepted(live, sid, None, None, Some(&a)));
    assert_eq!(store.undelivered_secret(live, sid), Some(a.clone()));
    assert_eq!(store.secret_for_poll(live, &sid.to_uppercase()), Some(a.clone()));
    assert_eq!(store.state(live, sid), ServerIdentityState::Known);
    assert_eq!(store.issued_entries().len(), 1);

    // Other backends never see it.
    assert_eq!(store.candidates_for_action("mock", sid, None), vec![None]);
    assert_eq!(store.secret_for_poll("mock", sid), None);

    // Queued actions carry none or an old value: the issued one wins.
    assert_eq!(
        store.candidates_for_action(live, sid, Some(&b)),
        vec![Some(a.clone()), Some(b.clone())]
    );
    assert!(!store.record_accepted(live, sid, Some(&a), None, None));
    assert_eq!(store.undelivered_secret(live, sid), Some(a.clone()));

    // The mod sends it: delivered, from now on the XML value is tried first.
    assert!(store.record_accepted(live, sid, Some(&a), Some(&a), None));
    assert_eq!(store.undelivered_secret(live, sid), None);
    assert_eq!(
        store.candidates_for_action(live, sid, Some(&b)),
        vec![Some(b.clone()), Some(a.clone())]
    );

    // Game closed without saving: the mod lost it, so it is delivered again.
    assert_eq!(store.candidates_for_action(live, sid, None), vec![Some(a.clone())]);
    assert!(store.record_accepted(live, sid, Some(&a), None, None));
    assert_eq!(store.undelivered_secret(live, sid), Some(a.clone()));

    // Rejected: reported once, no authenticated poll, no delivery, but kept as a fallback.
    assert!(store.record_rejected(live, sid));
    assert!(!store.record_rejected(live, sid));
    assert_eq!(store.state(live, sid), ServerIdentityState::Rejected);
    assert_eq!(store.secret_for_poll(live, sid), None);
    assert_eq!(store.undelivered_secret(live, sid), None);
    assert_eq!(
        store.candidates_for_action(live, sid, Some(&b)),
        vec![Some(b.clone()), Some(a.clone())]
    );
    assert!(store.record_accepted(live, sid, Some(&b), Some(&b), None));
    assert_eq!(store.state(live, sid), ServerIdentityState::Known);
    assert_eq!(store.secret_for_poll(live, sid), Some(b.clone()));

    // A secret that only came from the mod is neither persisted nor passed on.
    let mut learned = ServerIdentityStore::default();
    assert!(!learned.record_accepted(live, sid, Some(&b), Some(&b), None));
    assert!(learned.issued_entries().is_empty());
    assert_eq!(learned.secret_for_poll(live, sid), Some(b.clone()));
    assert!(!learned.record_accepted(live, sid, Some(&b), None, None));
    assert_eq!(learned.undelivered_secret(live, sid), None);

    // A rejection without any known secret clears with the next accepted action.
    let mut unknown = ServerIdentityStore::default();
    assert!(unknown.record_rejected(live, sid));
    assert_eq!(unknown.state(live, sid), ServerIdentityState::Rejected);
    unknown.record_accepted(live, sid, None, None, Some(&a));
    assert_eq!(unknown.state(live, sid), ServerIdentityState::Known);
}

#[test]
fn contract_xml_carries_server_secret_identity_and_results_only_files() {
    let dir = env::temp_dir().join(format!("fs25_gm_identity_xml_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let (staging, read_dir) = (dir.join(".staging"), dir.join("read"));

    let xml = r#"<gmDataSync action="contractAction">
    <request subAction="REQUEST_SYNC" actionId="x_1" serverId="a3f901c48b2e7d10" serverSecret="s3cr-et_1" />
    <request subAction="REQUEST_SYNC" actionId="x_2" serverId="a3f901c48b2e7d10" serverSecret="" />
    <request subAction="REQUEST_SYNC" actionId="x_3" serverId="a3f901c48b2e7d10" />
</gmDataSync>"#;
    match parse_incoming_write_xml(xml, "1_1").unwrap() {
        IncomingPacket::ContractActions(reqs) => {
            assert_eq!(reqs.len(), 3);
            assert_eq!(reqs[0].server_secret, Some(secret("s3cr-et_1")));
            assert_eq!(reqs[1].server_secret, None);
            assert_eq!(reqs[2].server_secret, None);
        }
        other => panic!("unexpected packet {other:?}"),
    }

    let state = ContractStateResponse::default();
    let with_identity = fs::read_to_string(
        write_contract_data_xml_with_identity(
            &staging,
            &read_dir,
            &state,
            Some((IDENTITY_SERVER_ID, &secret("s3cr-et_1"))),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(with_identity.contains(
        "<serverIdentity serverId=\"a3f901c48b2e7d10\" serverSecret=\"s3cr-et_1\" />"
    ));
    assert!(with_identity.contains("<contracts>"));

    let without =
        fs::read_to_string(write_contract_data_xml(&staging, &read_dir, &state).unwrap()).unwrap();
    assert!(!without.contains("serverIdentity"));

    let results_only = fs::read_to_string(
        write_contract_results_xml(
            &staging,
            &read_dir,
            &[ContractActionResultDto {
                action_id: "x_1".to_string(),
                sub_action: "CREATE_OFFER".to_string(),
                status: "REJECTED".to_string(),
                reason: "SERVER_SECRET_INVALID".to_string(),
                remote_id: String::new(),
                granted_liters: 0,
                base_trend_factor: 1.0,
                bonus_pct: 0,
                remaining_seconds: 0,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    assert!(results_only.contains("reason=\"SERVER_SECRET_INVALID\""));
    for section in ["<contracts", "<directOffers", "<settlements"] {
        assert!(!results_only.contains(section), "{section} would clear the mod's board");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mock_backend_follows_the_server_secret_rules() {
    let mock = LocalMockBackend::new();
    let submit = |id: &str, s: Option<&ServerSecret>| {
        mock.submit_contract_action(sync_request(id, None), s)
    };
    let sid = Some(IDENTITY_SERVER_ID);

    // Reads never register: an unknown savegame gets the board only.
    assert!(mock.fetch_contracts_state(sid, None).unwrap().server_secret.is_none());

    let first = submit("a1", None).unwrap().server_secret.expect("issued");
    // First response lost: a new secret until one was used.
    let second = submit("a2", None).unwrap().server_secret.expect("issued again");
    assert_ne!(first, second);
    assert!(matches!(
        mock.fetch_contracts_state(sid, Some(&first)),
        Err(ApiError::ServerSecretInvalid)
    ));
    let own = mock.fetch_contracts_state(sid, Some(&second)).unwrap();
    assert!(own.server_secret.is_none());
    // Other savegames' ids are never sent.
    assert!(own.contracts.iter().all(|c| c.issuer_server_id.is_empty()));
    assert!(own.direct_offers.iter().all(|o| o.seller_server_id.is_empty()));

    assert!(matches!(submit("a3", None), Err(ApiError::ServerSecretInvalid)));
    assert!(submit("a4", Some(&second)).unwrap().server_secret.is_none());

    // Support reset: a savegame that still has its secret keeps using it.
    mock.reset_server_identities();
    assert!(submit("a5", Some(&second)).unwrap().server_secret.is_none());
    assert!(matches!(submit("a6", None), Err(ApiError::ServerSecretInvalid)));
}

/// Minimal contracts backend: accepts only `good_secret`, records `METHOD path [secret]` per request.
fn spawn_identity_server(
    good_secret: &'static str,
    issue: bool,
) -> (String, Arc<Mutex<Vec<String>>>, Arc<AtomicBool>, thread::JoinHandle<()>) {
    spawn_identity_server_with(good_secret, issue, false)
}

/// Like [`spawn_identity_server`]; with `vehicles` it also offers the vehicle market (API 2.9.0).
fn spawn_identity_server_with(
    good_secret: &'static str,
    issue: bool,
    vehicles: bool,
) -> (String, Arc<Mutex<Vec<String>>>, Arc<AtomicBool>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (log, stop_flag) = (Arc::clone(&requests), Arc::clone(&stop));
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::SeqCst) {
            let Ok((mut stream, _)) = listener.accept() else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let (mut length, mut server_secret, mut none_match) = (0_usize, String::new(), String::new());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = line.trim().split_once(':') {
                    match k.to_ascii_lowercase().as_str() {
                        "content-length" => length = v.trim().parse().unwrap_or(0),
                        "x-server-secret" => server_secret = v.trim().to_string(),
                        "if-none-match" => none_match = v.trim().to_string(),
                        _ => {}
                    }
                }
            }
            let mut body = vec![0_u8; length];
            let _ = reader.read_exact(&mut body);
            let target = request_line
                .rsplit_once(' ')
                .map(|(t, _)| t.to_string())
                .unwrap_or_default();
            assert!(
                !String::from_utf8_lossy(&body).contains("secret"),
                "the secret belongs into the header only"
            );
            log.lock().unwrap().push(format!("{target} [{server_secret}]"));

            const BOARD: &str = r#""updated_at":1,"contracts":[{"remote_id":"c_1","issuer_server_id":"","issuer_farm_id":1,"issuer_name":"Nordhof","fill_type":"WHEAT","total_liters":1000,"bonus_pct":10,"remaining_seconds":3600,"status":"OPEN"}],"direct_offers":[],"action_results":[],"settlements":[],"listings":[{"remote_id":"v_9","create_action_id":"","sale_mode":"FIXED","seller_server_id":"","seller_farm_id":1,"seller_name":"Nordhof","price_factor":1.1,"remaining_seconds":3600,"status":"OPEN","vehicle":{"xml_filename":"data/x.xml","mod_name":null,"group":"DRIVABLES","age_months":3,"operating_hours":1.5,"damage":0.1,"wear":0.2,"configurations":[{"name":"baseColor","save_id":"4","is_active":true,"is_bought":true,"color":[0.62,0.05,0.04],"material_template":null}]}}]"#;
            let forbidden = r#"{"detail":{"code":"SERVER_SECRET_INVALID"}}"#.to_string();
            let (status, json) = if target.contains("/meta/status") {
                ("200 OK", format!(r#"{{"online":true,"min_exe_version":"0.9.9","poll_interval_sec":1,"market_name":"Global Market","api_version":"2.8.0","contracts_supported":true,"vehicles_supported":{vehicles}}}"#))
            } else if target.contains("/market/") {
                ("200 OK", r#"{"updated_at":1,"items":[]}"#.to_string())
            } else if target.contains("/contracts/state") || target.contains("/vehicles/state") {
                if target.contains("server_id=") && server_secret != good_secret {
                    ("403 Forbidden", forbidden)
                } else if !none_match.is_empty() {
                    ("304 Not Modified", String::new())
                } else {
                    ("200 OK", format!("{{{BOARD}}}"))
                }
            } else if server_secret == good_secret {
                ("200 OK", format!("{{{BOARD}}}"))
            } else if issue && server_secret.is_empty() {
                ("200 OK", format!("{{{BOARD},\"server_secret\":\"{good_secret}\"}}"))
            } else {
                ("403 Forbidden", forbidden)
            };
            let etag = if target.contains("/contracts/") || target.contains("/vehicles/") { "ETag: \"e1\"\r\n" } else { "" };
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{etag}Content-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                )
                .as_bytes(),
            );
            let _ = stream.flush();
        }
    });
    (url, requests, stop, handle)
}

#[test]
fn fastapi_sends_server_secret_and_maps_403_without_leaking_it() {
    let (url, requests, stop, handle) = spawn_identity_server("good-secret", true);
    let api = FastApiBackend::new(url, None, "a1b2c3d4e5f60718".to_string()).unwrap();
    let sid = Some(IDENTITY_SERVER_ID);

    let issued = api
        .submit_contract_action(sync_request("a1", None), None)
        .unwrap()
        .server_secret
        .expect("secret of the registration");
    assert_eq!(issued.expose(), "good-secret");

    // The POST state is cached: the 304 that follows must not look like a newly issued secret.
    let polled = api.fetch_contracts_state(sid, Some(&issued)).unwrap();
    assert_eq!(polled.contracts.len(), 1);
    assert!(polled.server_secret.is_none());

    let wrong = secret("wrong-secret");
    for err in [
        api.fetch_contracts_state(sid, Some(&wrong)).unwrap_err(),
        api.fetch_contracts_state(sid, None).unwrap_err(),
        api.submit_contract_action(sync_request("a2", None), Some(&wrong))
            .unwrap_err(),
    ] {
        assert!(matches!(err, ApiError::ServerSecretInvalid), "{err:?}");
        assert!(!format!("{err} {err:?}").contains("wrong-secret"));
    }

    // Without `server_id` the passed secret is not sent; the remembered savegame identity of the
    // last successful board request goes along instead (#14).
    api.fetch_contracts_state(None, Some(&wrong)).unwrap();
    let log = requests.lock().unwrap().clone();
    assert_eq!(log[0], "POST /api/v1/contracts/actions []");
    assert_eq!(
        log[1],
        format!("GET /api/v1/contracts/state?server_id={IDENTITY_SERVER_ID} [good-secret]")
    );
    assert_eq!(log.last().unwrap(), "GET /api/v1/contracts/state [good-secret]");
    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
}

fn identity_test_profile(name: &str, config: AppConfig) -> (PathBuf, PathBuf, PathBuf) {
    let test_dir = env::temp_dir().join(format!("fs25_gm_identity_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&test_dir);
    let profile_dir = test_dir.join("FarmingSimulator2025");
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(profile_dir.join("gameSettings.xml"), "<gameSettings/>").unwrap();
    let config_file = test_dir.join("config").join("settings.json");
    AppConfig {
        access_tier: AccessTier::Developer,
        language: "en".to_string(),
        fs25_profile_path: Some(profile_dir.to_string_lossy().into_owned()),
        ..config
    }
    .save_to_file(&config_file)
    .unwrap();
    let bridge_root = profile_dir.join("modSettings").join("FS25_GlobalMarket");
    (test_dir, config_file, bridge_root)
}

fn wait_for_contracts(bridge_root: &std::path::Path) {
    for _ in 0..200 {
        if fs::read_to_string(bridge_root.join("globalMarketOnline.xml"))
            .is_ok_and(|s| s.contains("contractsSupported=\"true\""))
        {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("contracts never became available");
}

#[test]
fn issued_secret_reaches_the_mod_and_survives_a_client_restart() {
    let (test_dir, config_file, bridge_root) = identity_test_profile(
        "restart",
        AppConfig {
            backend_mode: BackendMode::Mock,
            ..AppConfig::default()
        },
    );
    let read_dir = bridge_root.join("read");
    let has_identity = |xml: &str| xml.contains("<serverIdentity ");

    let mgr = BridgeManager::new(config_file.clone(), None);
    wait_for_contracts(&bridge_root);
    thread::sleep(Duration::from_millis(800));
    assert!(
        read_contract_sync_xmls(&read_dir).is_empty(),
        "a board without identity must not reach the mod"
    );
    assert!(mgr.get_snapshot().open_contracts_count > 0, "the UI still counts the board");

    // Registration: the answer carries the secret, and so does every sync until the mod sends it.
    let xml = send_action_xml(
        &bridge_root,
        1,
        "subAction=\"REQUEST_SYNC\" actionId=\"i_1\" serverSecret=\"\"",
        &has_identity,
    );
    assert_eq!(
        extract_attr(&xml, "serverIdentity", "serverId").as_deref(),
        Some(IDENTITY_SERVER_ID)
    );
    let issued = extract_attr(&xml, "serverIdentity", "serverSecret").unwrap();
    clear_dir(&read_dir);
    let xml = send_action_xml(
        &bridge_root,
        2,
        "subAction=\"CREATE_OFFER\" actionId=\"i_2\" farmId=\"1\" fillType=\"WHEAT\" liters=\"1000\" priceDeltaPct=\"0\" durationHours=\"24\"",
        &|xml| xml.contains("actionId=\"i_2\""),
    );
    assert!(xml.contains("actionId=\"i_2\" subAction=\"CREATE_OFFER\" status=\"ACCEPTED\""));
    assert!(xml.contains(&issued));
    assert_eq!(mgr.get_snapshot().server_identity, "known");

    let report = mgr.generate_diagnostic_report();
    assert!(report.contains("server_identity: \"known\""));
    assert!(!report.contains(&issued));
    assert!(!serde_json::to_string(&mgr.get_snapshot()).unwrap().contains(&issued));

    // Client exits with the file unread: the secret comes from the vault, not from `read/`.
    mgr.shutdown();
    drop(mgr);
    assert!(read_contract_sync_xmls(&read_dir).is_empty());
    let vault_file = config_file.with_file_name("tokens.vault");
    assert!(vault_file.is_file());
    #[cfg(target_os = "windows")]
    assert!(!String::from_utf8_lossy(&fs::read(&vault_file).unwrap()).contains(&issued));
    assert!(!fs::read_to_string(&config_file).unwrap().contains(&issued));

    let mgr = BridgeManager::new(config_file, None);
    wait_for_contracts(&bridge_root);
    let xml = send_action_xml(
        &bridge_root,
        3,
        "subAction=\"REQUEST_SYNC\" actionId=\"i_3\" serverSecret=\"\"",
        &has_identity,
    );
    assert_eq!(extract_attr(&xml, "serverIdentity", "serverSecret"), Some(issued.clone()));

    // The mod has it now: no further delivery.
    clear_dir(&read_dir);
    let xml = send_action_xml(
        &bridge_root,
        4,
        &format!("subAction=\"REQUEST_SYNC\" actionId=\"i_4\" serverSecret=\"{issued}\""),
        &|xml| xml.contains("<contracts>"),
    );
    assert!(!has_identity(&xml));
    mgr.shutdown();
    let _ = fs::remove_dir_all(&test_dir);
}

#[test]
fn rejected_identity_is_passed_to_the_mod_and_recovers() {
    let (url, requests, stop, handle) = spawn_identity_server("good-secret", false);
    let (test_dir, config_file, bridge_root) = identity_test_profile(
        "reject",
        AppConfig {
            backend_mode: BackendMode::Live,
            backend_url: url,
            ..AppConfig::default()
        },
    );
    let read_dir = bridge_root.join("read");
    let mgr = BridgeManager::new(config_file, None);
    wait_for_contracts(&bridge_root);

    for (seq, sub) in [(1, "REQUEST_SYNC"), (2, "CREATE_CONTRACT")] {
        let xml = send_action_xml(
            &bridge_root,
            seq,
            &format!("subAction=\"{sub}\" actionId=\"r_{seq}\" farmId=\"1\" fillType=\"WHEAT\" liters=\"5000\" bonusPct=\"10\" durationHours=\"24\" serverSecret=\"wrong-secret\""),
            &|xml| xml.contains(&format!("actionId=\"r_{seq}\"")),
        );
        assert!(xml.contains(&format!(
            "actionId=\"r_{seq}\" subAction=\"{sub}\" status=\"REJECTED\" reason=\"SERVER_SECRET_INVALID\""
        )));
        assert!(!xml.contains("<contracts"), "must not clear the mod's board");
        clear_dir(&read_dir);
    }

    let snap = mgr.get_snapshot();
    assert_eq!(snap.server_identity, "rejected");
    assert_eq!(snap.state, BridgeConnectionState::Online, "not a connection problem");
    let rejected_logs = mgr
        .get_logs()
        .iter()
        .filter(|l| l.key.as_deref() == Some("logServerIdentityRejected"))
        .count();
    assert_eq!(rejected_logs, 1, "logged once");
    let report = mgr.generate_diagnostic_report();
    assert!(report.contains("rejected (savegame identity rejected, contact support)"));
    assert!(!report.contains("wrong-secret"));

    // Board only while rejected, and nothing of it reaches the mod.
    thread::sleep(Duration::from_millis(1800));
    {
        let log = requests.lock().unwrap();
        assert!(log.iter().any(|r| r == "GET /api/v1/contracts/state []"));
        assert!(!log.iter().any(|r| r.contains("state?server_id=")));
        assert_eq!(log.iter().filter(|r| r.starts_with("POST /api/v1/contracts")).count(), 2);
    }
    assert!(read_contract_sync_xmls(&read_dir).is_empty());

    // The right secret recovers, and is used for polling.
    let xml = send_action_xml(
        &bridge_root,
        3,
        "subAction=\"REQUEST_SYNC\" actionId=\"r_3\" serverSecret=\"good-secret\"",
        &|xml| xml.contains("remoteId=\"c_1\""),
    );
    assert!(!xml.contains("serverIdentity"), "only issued secrets are passed on");
    assert_eq!(mgr.get_snapshot().server_identity, "known");
    let expected = format!("GET /api/v1/contracts/state?server_id={IDENTITY_SERVER_ID} [good-secret]");
    for _ in 0..100 {
        if requests.lock().unwrap().contains(&expected) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(requests.lock().unwrap().contains(&expected));

    mgr.shutdown();
    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    let _ = fs::remove_dir_all(&test_dir);
}

// ---------------------------------------------------------------------------
// Stage 3 used vehicle market (`vehicleAction` / `vehicleData`, Spec Section 3.6)
// ---------------------------------------------------------------------------

use globalmarket_client_lib::api::types::{
    OnlineVehicleListingDto, VehicleActionRequest, VehicleActionResultDto, VehicleConfigDto,
    VehicleDto, VehicleSettlementDto, VehicleStateResponse, VehicleSubAction,
};
use globalmarket_client_lib::bridge::xml::{
    cleanup_bridge_transfer_files, prune_market_data_xmls, write_vehicle_data_xml,
    write_vehicle_data_xml_with_identity, write_vehicle_results_xml,
};

fn painted_vehicle() -> VehicleDto {
    VehicleDto {
        xml_filename: "$moddir$FS25_ExampleTractor/tractor.xml".to_string(),
        mod_name: Some("FS25_ExampleTractor".to_string()),
        mod_file_hash: Some("3f1ce9".to_string()),
        group: "DRIVABLES".to_string(),
        age_months: 26,
        operating_hours: 412.5,
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

fn vehicle_request(sub: VehicleSubAction, action_id: &str) -> VehicleActionRequest {
    VehicleActionRequest {
        sub_action: sub,
        action_id: action_id.to_string(),
        server_id: IDENTITY_SERVER_ID.to_string(),
        farm_id: Some(1),
        farm_name: Some("Hof Meier".to_string()),
        remote_id: None,
        create_action_id: None,
        sale_mode: None,
        price_factor: None,
        duration_hours: None,
        settlement_ids: None,
        vehicle: None,
        configs_truncated: false,
        server_secret: None,
    }
}

fn create_listing_request(action_id: &str) -> VehicleActionRequest {
    VehicleActionRequest {
        sale_mode: Some("FIXED".to_string()),
        price_factor: Some(1.10),
        duration_hours: Some(72),
        vehicle: Some(painted_vehicle()),
        ..vehicle_request(VehicleSubAction::CreateListing, action_id)
    }
}

fn read_sync_xmls(read_dir: &std::path::Path, prefix: &str) -> Vec<String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(read_dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    paths.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(prefix))
    });
    paths.sort();
    paths
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect()
}

/// Writes a `vehicleAction_*.xml` like the mod and waits until the client answered it.
fn send_vehicle_xml(
    bridge_root: &std::path::Path,
    seq: u32,
    requests: &str,
    answered: &dyn Fn(&str) -> bool,
) -> String {
    let write_dir = bridge_root.join("write");
    let read_dir = bridge_root.join("read");
    let tmp = write_dir.join(format!("vehicleAction_20261125_120000_{seq}.tmp"));
    fs::write(
        &tmp,
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"no\"?>\n<gmDataSync action=\"vehicleAction\">\n{requests}\n</gmDataSync>"
        ),
    )
    .unwrap();
    fs::rename(&tmp, tmp.with_extension("xml")).unwrap();
    for _ in 0..200 {
        let joined = read_sync_xmls(&read_dir, "gmVehicleSync_").join("\n");
        if fs::read_dir(&write_dir).unwrap().flatten().count() == 0 && answered(&joined) {
            return joined;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "vehicle action {seq} was not answered, read/: {:?}",
        read_sync_xmls(&read_dir, "gmVehicleSync_")
    );
}

fn wait_for_heartbeat_flag(bridge_root: &std::path::Path, flag: &str) {
    for _ in 0..200 {
        if fs::read_to_string(bridge_root.join("globalMarketOnline.xml"))
            .is_ok_and(|s| s.contains(flag))
        {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("heartbeat never contained {flag}");
}

#[test]
fn stage3_vehicle_action_xml_parsing_with_nested_vehicle_and_paint() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
<gmDataSync action="vehicleAction">
    <request subAction="CREATE_LISTING" actionId="a3f901c48b2e7d10_15000_1" serverId="a3f901c48b2e7d10" serverSecret="s3cr-et_1" farmId="1" farmName="Hof Meier [Dedi EU]" saleMode="FIXED" priceFactor="1,10" durationHours="72">
        <vehicle xmlFilename="$moddir$FS25_ExampleTractor/tractor.xml" modName="FS25_ExampleTractor" modFileHash="3f1c...e9" group="DRIVABLES" ageMonths="26" operatingHours="412.5" damage="0.18" wear="0.31">
            <config name="wheel" saveId="3" isActive="true" isBought="true" />
            <config name="baseColor" saveId="4" isActive="true" isBought="true" color="0.62 0.05 0.04" materialTemplate="" />
            <config name="design" saveId="2" isActive="false" isBought="true" color="broken" />
        </vehicle>
    </request>
    <request subAction="CANCEL_LISTING" actionId="c_1" serverId="a3f901c48b2e7d10" createActionId="a3f901c48b2e7d10_9000_3" />
    <request subAction="BUY_LISTING" actionId="b_1" serverId="a3f901c48b2e7d10" farmId="2" remoteId="v_301"></request>
    <request subAction="ACK_SETTLEMENT" serverId="a3f901c48b2e7d10" settlementIds="vs_77,vs_78" />
    <request subAction="REQUEST_SYNC" actionId="s_1" serverId="a3f901c48b2e7d10" serverSecret="" />
    <request subAction="AUCTION_BID" actionId="x_1" serverId="a3f901c48b2e7d10" />
    <request subAction="REQUEST_SYNC" actionId="x_2" />
</gmDataSync>"#;
    let reqs = match parse_incoming_write_xml(xml, "20261125_7").unwrap() {
        IncomingPacket::VehicleActions(reqs) => reqs,
        other => panic!("unexpected packet {other:?}"),
    };
    // Unknown sub actions and requests without server id are dropped.
    assert_eq!(reqs.len(), 5);

    let create = &reqs[0];
    assert_eq!(create.sub_action, VehicleSubAction::CreateListing);
    assert_eq!(create.server_secret, Some(secret("s3cr-et_1")));
    assert_eq!(create.farm_name.as_deref(), Some("Hof Meier [Dedi EU]"));
    assert_eq!(create.price_factor, Some(1.10));
    assert_eq!(create.duration_hours, Some(72));
    let vehicle = create.vehicle.as_ref().expect("nested vehicle");
    assert_eq!(vehicle.mod_name.as_deref(), Some("FS25_ExampleTractor"));
    assert_eq!(vehicle.age_months, 26);
    assert_eq!(vehicle.operating_hours, 412.5);
    assert_eq!(vehicle.configurations.len(), 3);
    assert_eq!(vehicle.configurations[1].color, Some([0.62, 0.05, 0.04]));
    assert_eq!(vehicle.configurations[1].material_template.as_deref(), Some(""));
    assert_eq!(vehicle.configurations[2].color, None, "broken paint counts as none");
    assert!(!vehicle.configurations[2].is_active);
    assert_eq!(create.local_rejection_reason(), None);

    assert_eq!(reqs[1].create_action_id.as_deref(), Some("a3f901c48b2e7d10_9000_3"));
    assert_eq!(reqs[1].remote_id, None);
    assert_eq!(reqs[2].remote_id.as_deref(), Some("v_301"));
    assert_eq!(reqs[2].farm_id, Some(2));
    assert_eq!(reqs[3].settlement_ids.as_deref(), Some("vs_77,vs_78"));
    assert_eq!(reqs[3].action_id, "a3f901c48b2e7d10_20261125_7_3", "derived from the file name");
    assert_eq!(reqs[4].server_secret, None);

    // The secret never shows up in debug output or JSON.
    assert!(!format!("{create:?}").contains("s3cr-et_1"));
    assert!(!serde_json::to_string(create).unwrap().contains("s3cr-et_1"));

    // More than 64 configurations: parsed up to the limit and rejected locally.
    let configs: String = (0..70)
        .map(|i| format!("<config name=\"c{i}\" saveId=\"{i}\" isActive=\"true\" isBought=\"true\" />"))
        .collect();
    let big = format!(
        "<gmDataSync action=\"vehicleAction\"><request subAction=\"CREATE_LISTING\" actionId=\"big_1\" serverId=\"a3f901c48b2e7d10\"><vehicle xmlFilename=\"data/x.xml\">{configs}</vehicle></request></gmDataSync>"
    );
    match parse_incoming_write_xml(&big, "1_1").unwrap() {
        IncomingPacket::VehicleActions(reqs) => {
            assert_eq!(reqs[0].vehicle.as_ref().unwrap().configurations.len(), 64);
            assert!(reqs[0].configs_truncated);
            assert_eq!(reqs[0].local_rejection_reason(), Some("INVALID_REQUEST"));
        }
        other => panic!("unexpected packet {other:?}"),
    }
    // CREATE_LISTING without a vehicle or with an oversized snapshot never leaves the client.
    let mut no_vehicle = create_listing_request("nv_1");
    no_vehicle.vehicle = None;
    assert_eq!(no_vehicle.local_rejection_reason(), Some("INVALID_REQUEST"));
    let mut huge = create_listing_request("h_1");
    huge.vehicle.as_mut().unwrap().configurations[0].name = "x".repeat(17 * 1024);
    assert_eq!(huge.local_rejection_reason(), Some("INVALID_REQUEST"));
    assert_eq!(vehicle_request(VehicleSubAction::RequestSync, "s").local_rejection_reason(), None);

    // Unknown future actions are still skipped.
    assert!(matches!(
        parse_incoming_write_xml("<gmDataSync action=\"vehicleAuction\" />", "1").unwrap(),
        IncomingPacket::IgnoredUnknownAction(a) if a == "vehicleAuction"
    ));
}

#[test]
fn stage3_vehicle_sync_xml_masks_ids_and_keeps_its_own_file_class() {
    let dir = env::temp_dir().join(format!("fs25_gm_vehicle_xml_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let (staging, read_dir) = (dir.join(".staging"), dir.join("read"));

    let foreign = OnlineVehicleListingDto {
        remote_id: "v_301".to_string(),
        create_action_id: String::new(),
        sale_mode: "FIXED".to_string(),
        seller_server_id: String::new(),
        seller_farm_id: 1,
        seller_name: "Nordhof & Söhne".to_string(),
        price_factor: 1.12,
        remaining_seconds: 259_200,
        status: "OPEN".to_string(),
        vehicle: painted_vehicle(),
    };
    let own = OnlineVehicleListingDto {
        remote_id: "v_302".to_string(),
        create_action_id: format!("{IDENTITY_SERVER_ID}_15000_1"),
        seller_server_id: IDENTITY_SERVER_ID.to_string(),
        vehicle: VehicleDto {
            mod_name: None,
            mod_file_hash: None,
            configurations: Vec::new(),
            ..painted_vehicle()
        },
        ..foreign.clone()
    };
    let state = VehicleStateResponse {
        updated_at: 1,
        listings: vec![foreign, own],
        action_results: vec![VehicleActionResultDto {
            action_id: "b_1".to_string(),
            sub_action: "BUY_LISTING".to_string(),
            status: "ACCEPTED".to_string(),
            reason: String::new(),
            remote_id: "v_301".to_string(),
            create_action_id: String::new(),
            remaining_seconds: 0,
            vehicle: Some(painted_vehicle()),
        }],
        settlements: vec![VehicleSettlementDto {
            settlement_id: "vs_78".to_string(),
            settlement_type: "LISTING_BOUGHT".to_string(),
            target_server_id: IDENTITY_SERVER_ID.to_string(),
            target_farm_id: 2,
            remote_id: "v_301".to_string(),
            create_action_id: String::new(),
            buy_action_id: "b_1".to_string(),
            price_factor: 1.12,
            buyer_name: String::new(),
            seller_name: "Nordhof".to_string(),
            vehicle: Some(painted_vehicle()),
        }],
        server_secret: Some(secret("never-written")),
    };

    let xml = fs::read_to_string(
        write_vehicle_data_xml_with_identity(
            &staging,
            &read_dir,
            &state,
            Some((IDENTITY_SERVER_ID, &secret("s3cr-et_1"))),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(xml.starts_with("<?xml"));
    assert!(xml.contains("<gmDataSync action=\"vehicleData\">"));
    assert!(xml.contains("<serverIdentity serverId=\"a3f901c48b2e7d10\" serverSecret=\"s3cr-et_1\" />"));
    assert!(!xml.contains("never-written"), "the issued secret only goes via serverIdentity");
    // Masked ids stay empty, own ids are passed through.
    assert!(xml.contains("<listing remoteId=\"v_301\" createActionId=\"\" saleMode=\"FIXED\" sellerServerId=\"\" sellerFarmId=\"1\" sellerName=\"Nordhof &amp; Söhne\" priceFactor=\"1.12\" remainingSeconds=\"259200\" status=\"OPEN\">"));
    assert!(xml.contains(&format!(
        "<listing remoteId=\"v_302\" createActionId=\"{IDENTITY_SERVER_ID}_15000_1\" saleMode=\"FIXED\" sellerServerId=\"{IDENTITY_SERVER_ID}\""
    )));
    assert!(xml.contains("<vehicle xmlFilename=\"$moddir$FS25_ExampleTractor/tractor.xml\" modName=\"FS25_ExampleTractor\" modFileHash=\"3f1ce9\" group=\"DRIVABLES\" ageMonths=\"26\" operatingHours=\"412.5\" damage=\"0.18\" wear=\"0.31\">"));
    assert!(xml.contains("<config name=\"baseColor\" saveId=\"4\" isActive=\"true\" isBought=\"true\" color=\"0.62 0.05 0.04\" materialTemplate=\"\" />"));
    assert!(xml.contains("<config name=\"wheel\" saveId=\"3\" isActive=\"true\" isBought=\"true\" />"));
    // Basegame vehicle without configurations: no mod attributes, self-closing.
    assert!(xml.contains("<vehicle xmlFilename=\"$moddir$FS25_ExampleTractor/tractor.xml\" group=\"DRIVABLES\" ageMonths=\"26\" operatingHours=\"412.5\" damage=\"0.18\" wear=\"0.31\" />"));
    assert!(xml.contains("<result actionId=\"b_1\" subAction=\"BUY_LISTING\" status=\"ACCEPTED\" reason=\"\" remoteId=\"v_301\" createActionId=\"\" remainingSeconds=\"0\">"));
    assert!(xml.contains("<settlement settlementId=\"vs_78\" type=\"LISTING_BOUGHT\" targetServerId=\"a3f901c48b2e7d10\" targetFarmId=\"2\" remoteId=\"v_301\" createActionId=\"\" buyActionId=\"b_1\" priceFactor=\"1.12\" buyerName=\"\" sellerName=\"Nordhof\">"));
    assert!(!xml.as_bytes().starts_with(&[0xEF, 0xBB, 0xBF]));
    // Well-formed: the parser accepts it (and ignores it as an unknown incoming action).
    assert!(matches!(
        parse_incoming_write_xml(&xml, "1").unwrap(),
        IncomingPacket::IgnoredUnknownAction(a) if a == "vehicleData"
    ));

    let without = fs::read_to_string(write_vehicle_data_xml(&staging, &read_dir, &state).unwrap()).unwrap();
    assert!(!without.contains("serverIdentity"));

    // Results only: a missing `<listings>` keeps the mod's board, an empty one would clear it.
    let results_only = fs::read_to_string(
        write_vehicle_results_xml(
            &staging,
            &read_dir,
            &[VehicleActionResultDto {
                action_id: "c_1".to_string(),
                sub_action: "CREATE_LISTING".to_string(),
                status: "REJECTED".to_string(),
                reason: "SERVER_SECRET_INVALID".to_string(),
                remote_id: String::new(),
                create_action_id: "c_1".to_string(),
                remaining_seconds: 0,
                vehicle: None,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    assert!(results_only.contains("<result actionId=\"c_1\" subAction=\"CREATE_LISTING\" status=\"REJECTED\" reason=\"SERVER_SECRET_INVALID\" remoteId=\"\" createActionId=\"c_1\" remainingSeconds=\"0\" />"));
    for section in ["<listings", "<settlements"] {
        assert!(!results_only.contains(section), "{section} must be missing");
    }

    // Own file class: market data retention never touches unread vehicle files.
    for _ in 0..(MAX_RETAINED_MARKET_SYNC_FILES + 2) {
        write_market_data_xml(&staging, &read_dir, &[]).unwrap();
    }
    prune_market_data_xmls(&read_dir, 0);
    assert_eq!(read_sync_xmls(&read_dir, "gmVehicleSync_").len(), 3);
    assert!(read_sync_xmls(&read_dir, "gmDataSync_").is_empty());
    cleanup_bridge_transfer_files(&read_dir, &staging);
    assert!(read_sync_xmls(&read_dir, "gmVehicleSync_").is_empty());

    // Capability flag only while connected.
    let online = dir.join("globalMarketOnline.xml");
    for (connected, flag, expected) in [(true, true, true), (true, false, false), (false, true, false)] {
        write_heartbeat_xml_with_extras(
            &staging,
            &online,
            connected,
            "0.9.16",
            "0.9.10",
            None,
            None,
            true,
            HeartbeatExtras {
                vehicles_supported: flag,
                ..Default::default()
            },
        )
        .unwrap();
        let hb = fs::read_to_string(&online).unwrap();
        assert_eq!(hb.contains("vehiclesSupported=\"true\""), expected, "{connected} {flag}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn stage3_mock_vehicle_market_follows_the_backend_rules() {
    let mock = LocalMockBackend::new();
    let sid = Some(IDENTITY_SERVER_ID);
    assert!(mock.check_status().unwrap().vehicles_supported);

    // Unknown savegame: board only, foreign ids masked.
    let board = mock.fetch_vehicles_state(sid, None).unwrap();
    assert!(board.listings.len() >= 2);
    assert!(board
        .listings
        .iter()
        .all(|l| l.seller_server_id.is_empty() && l.create_action_id.is_empty()));
    assert!(board.server_secret.is_none());
    let painted = board
        .listings
        .iter()
        .find(|l| l.vehicle.configurations.iter().any(|c| c.color.is_some()))
        .expect("a listing with custom paint");
    assert!(painted.vehicle.mod_name.is_some());

    // Registration on the vehicle board: the same secret then works for contracts.
    let created = mock
        .submit_vehicle_action(create_listing_request("cr_1"), None)
        .unwrap();
    let issued = created.server_secret.clone().expect("secret issued on registration");
    let res = &created.action_results[0];
    assert_eq!((res.status.as_str(), res.create_action_id.as_str()), ("ACCEPTED", "cr_1"));
    let own_rid = res.remote_id.clone();
    let own = created.listings.iter().find(|l| l.remote_id == own_rid).unwrap();
    assert_eq!(own.seller_server_id, IDENTITY_SERVER_ID);
    assert_eq!(own.create_action_id, "cr_1");
    assert_eq!(own.vehicle.configurations[1].color, Some([0.62, 0.05, 0.04]));
    assert!(mock.fetch_contracts_state(sid, Some(&issued)).is_ok(), "one secret for both boards");
    assert!(matches!(
        mock.fetch_vehicles_state(sid, Some(&secret("wrong"))),
        Err(ApiError::ServerSecretInvalid)
    ));
    let submit = |req: VehicleActionRequest| mock.submit_vehicle_action(req, Some(&issued)).unwrap();
    let result_of = |state: &VehicleStateResponse, id: &str| {
        state
            .action_results
            .iter()
            .find(|r| r.action_id == id)
            .cloned()
            .unwrap_or_else(|| panic!("no result for {id}"))
    };

    // Idempotent: a repeated action id changes nothing.
    let replay = submit(create_listing_request("cr_1"));
    assert_eq!(replay.listings.len(), created.listings.len());
    assert!(replay.server_secret.is_none());

    // Validation and limits.
    let mut bad = create_listing_request("cr_bad");
    bad.price_factor = Some(1.5);
    assert_eq!(result_of(&submit(bad), "cr_bad").reason, "INVALID_REQUEST");
    for i in 2..=20 {
        assert_eq!(result_of(&submit(create_listing_request(&format!("cr_{i}"))), &format!("cr_{i}")).status, "ACCEPTED");
    }
    assert_eq!(result_of(&submit(create_listing_request("cr_21")), "cr_21").reason, "LIMIT_REACHED");

    // Own listing cannot be bought by the same farm.
    let buy_own = VehicleActionRequest {
        remote_id: Some(own_rid.clone()),
        ..vehicle_request(VehicleSubAction::BuyListing, "buy_own")
    };
    assert_eq!(result_of(&submit(buy_own), "buy_own").reason, "OWN_LISTING");

    // Buying a foreign listing: vehicle in the result plus a LISTING_BOUGHT settlement.
    let foreign_rid = board.listings[0].remote_id.clone();
    let bought = submit(VehicleActionRequest {
        farm_id: Some(2),
        remote_id: Some(foreign_rid.clone()),
        ..vehicle_request(VehicleSubAction::BuyListing, "buy_1")
    });
    let res = result_of(&bought, "buy_1");
    assert_eq!(res.status, "ACCEPTED");
    assert!(res.vehicle.is_some());
    let settlement = bought
        .settlements
        .iter()
        .find(|s| s.settlement_type == "LISTING_BOUGHT")
        .expect("LISTING_BOUGHT");
    assert!(settlement.settlement_id.starts_with("vs_"));
    assert_eq!((settlement.buy_action_id.as_str(), settlement.target_farm_id), ("buy_1", 2));
    assert!(settlement.vehicle.is_some() && !settlement.seller_name.is_empty());
    assert!(bought.listings.iter().all(|l| l.remote_id != foreign_rid));
    let again = submit(VehicleActionRequest {
        farm_id: Some(2),
        remote_id: Some(foreign_rid),
        ..vehicle_request(VehicleSubAction::BuyListing, "buy_2")
    });
    assert_eq!(result_of(&again, "buy_2").reason, "LISTING_SOLD");

    // Settlements stay until ACK_SETTLEMENT.
    let acked = submit(VehicleActionRequest {
        settlement_ids: Some(settlement.settlement_id.clone()),
        ..vehicle_request(VehicleSubAction::AckSettlement, "ack_1")
    });
    assert!(acked.settlements.is_empty());

    // Cancel: the vehicle comes back, a repeated cancel answers again with the vehicle.
    for id in ["cn_1", "cn_2"] {
        let cancel = submit(VehicleActionRequest {
            remote_id: Some(own_rid.clone()),
            ..vehicle_request(VehicleSubAction::CancelListing, id)
        });
        let res = result_of(&cancel, id);
        assert_eq!((res.status.as_str(), res.reason.as_str()), ("ACCEPTED", ""), "{id}");
        assert_eq!(res.create_action_id, "cr_1");
        assert!(res.vehicle.is_some(), "{id}");
    }
    // Cancel by a create id that never arrived: ACCEPTED/NOT_CREATED, a late create is rejected.
    let never = submit(VehicleActionRequest {
        create_action_id: Some("lost_1".to_string()),
        ..vehicle_request(VehicleSubAction::CancelListing, "cn_3")
    });
    let res = result_of(&never, "cn_3");
    assert_eq!((res.status.as_str(), res.reason.as_str()), ("ACCEPTED", "NOT_CREATED"));
    assert!(res.vehicle.is_none());
    let late = submit(create_listing_request("lost_1"));
    assert_eq!(result_of(&late, "lost_1").reason, "NOT_CREATED");
    // Another farm cannot cancel.
    let mut other_farm = vehicle_request(VehicleSubAction::CancelListing, "cn_4");
    other_farm.remote_id = Some(result_of(&late, "cr_2").remote_id);
    other_farm.farm_id = Some(3);
    assert_eq!(result_of(&submit(other_farm), "cn_4").reason, "NOT_OWNER");

    // Expiry: LISTING_EXPIRED with the vehicle, a later cancel gets EXPIRED.
    mock.expire_all_vehicle_listings();
    let state = mock.fetch_vehicles_state(sid, Some(&issued)).unwrap();
    let expired: Vec<_> = state
        .settlements
        .iter()
        .filter(|s| s.settlement_type == "LISTING_EXPIRED")
        .collect();
    assert_eq!(expired.len(), 19);
    assert!(expired.iter().all(|s| s.vehicle.is_some() && !s.create_action_id.is_empty()));
    let cancel_expired = submit(VehicleActionRequest {
        remote_id: Some(expired[0].remote_id.clone()),
        ..vehicle_request(VehicleSubAction::CancelListing, "cn_5")
    });
    assert_eq!(result_of(&cancel_expired, "cn_5").reason, "EXPIRED");
    assert!(state.listings.iter().all(|l| l.seller_server_id.is_empty()));

    // Disabled market.
    mock.set_simulated_vehicles_supported(false);
    assert!(!mock.check_status().unwrap().vehicles_supported);
    assert!(matches!(mock.fetch_vehicles_state(None, None), Err(ApiError::NotSupported(_))));
}

/// Serves `responses` in order (`status`, extra headers, body) and returns every raw request.
fn spawn_scripted_server(
    responses: Vec<(&'static str, &'static str, String)>,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut seen = Vec::new();
        for (status, headers, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0_usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = line.trim().split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                head.push_str(&line);
            }
            let mut req_body = vec![0_u8; length];
            let _ = reader.read_exact(&mut req_body);
            seen.push(format!("{head}\n{}", String::from_utf8_lossy(&req_body)));
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = stream.flush();
        }
        seen
    });
    (url, handle)
}

#[test]
fn stage3_fastapi_maps_vehicle_json_both_ways() {
    let state_json = r#"{"updated_at":5,"server_secret":"issued-1",
        "listings":[{"remote_id":"v_301","create_action_id":"","sale_mode":"FIXED","seller_server_id":"","seller_farm_id":1,"seller_name":"Nordhof","price_factor":1.12,"remaining_seconds":259200,"status":"OPEN",
            "vehicle":{"xml_filename":"data/vehicles/fendt/vario700/vario700.xml","mod_name":null,"mod_file_hash":null,"group":"DRIVABLES","age_months":14,"operating_hours":230.0,"damage":0.05,"wear":0.12,
                "configurations":[{"name":"baseColor","save_id":"4","is_active":true,"is_bought":true,"color":[0.62,0.05,0.04],"material_template":null},{"name":"motor","save_id":2,"is_active":true,"is_bought":true,"color":null}]}},
            {"remote_id":"v_302","create_action_id":"","sale_mode":"FIXED","seller_server_id":"","seller_farm_id":1,"seller_name":"X","price_factor":1.0,"remaining_seconds":1,"status":"OPEN","vehicle":null}],
        "action_results":[{"action_id":"a3f901c48b2e7d10_15000_1","sub_action":"CREATE_LISTING","status":"ACCEPTED","reason":"","remote_id":"v_303","create_action_id":"a3f901c48b2e7d10_15000_1","remaining_seconds":259200,"vehicle":null}],
        "settlements":[{"settlement_id":"vs_77","type":"LISTING_SOLD","target_server_id":"a3f901c48b2e7d10","target_farm_id":1,"remote_id":"v_288","create_action_id":"a3f901c48b2e7d10_9000_3","buy_action_id":"","price_factor":1.05,"buyer_name":"Südhof","seller_name":"","vehicle":null}]}"#;
    let forbidden = r#"{"detail":{"code":"SERVER_SECRET_INVALID"}}"#.to_string();
    let (url, handle) = spawn_scripted_server(vec![
        ("200 OK", "ETag: \"v1\"\r\n", state_json.to_string()),
        ("304 Not Modified", "", String::new()),
        ("403 Forbidden", "", forbidden),
        ("404 Not Found", "", r#"{"detail":"Not Found"}"#.to_string()),
    ]);
    let api = FastApiBackend::new(url, None, "a1b2c3d4e5f60718".to_string()).unwrap();

    let mut req = create_listing_request("a3f901c48b2e7d10_15000_1");
    req.server_secret = Some(secret("from-the-mod"));
    let state = api.submit_vehicle_action(req, None).unwrap();
    assert_eq!(state.server_secret, Some(secret("issued-1")));
    assert_eq!(state.listings.len(), 1, "listings without vehicle are dropped");
    let listing = &state.listings[0];
    assert_eq!((listing.seller_server_id.as_str(), listing.create_action_id.as_str()), ("", ""));
    assert_eq!(listing.vehicle.mod_name, None);
    assert_eq!(listing.vehicle.configurations[0].color, Some([0.62, 0.05, 0.04]));
    assert_eq!(listing.vehicle.configurations[1].save_id, "2", "numeric save ids are accepted");
    assert_eq!(state.action_results[0].create_action_id, "a3f901c48b2e7d10_15000_1");
    assert_eq!(state.settlements[0].buyer_name, "Südhof");
    assert_eq!(state.settlements[0].create_action_id, "a3f901c48b2e7d10_9000_3");

    // The POST state is cached for the `server_id`: a 304 returns it without the issued secret.
    let polled = api
        .fetch_vehicles_state(Some(IDENTITY_SERVER_ID), Some(&secret("issued-1")))
        .unwrap();
    assert_eq!(polled.listings.len(), 1);
    assert!(polled.server_secret.is_none());

    let err = api
        .fetch_vehicles_state(Some(IDENTITY_SERVER_ID), Some(&secret("wrong-secret")))
        .unwrap_err();
    assert!(matches!(err, ApiError::ServerSecretInvalid));
    assert!(matches!(
        api.fetch_vehicles_state(None, None),
        Err(ApiError::NotSupported(_))
    ));

    let seen = handle.join().unwrap();
    let (post_head, post_body) = seen[0].split_once("\n\n").unwrap();
    assert!(post_head.starts_with("POST /api/v1/vehicles/actions "));
    assert!(!post_head.to_ascii_lowercase().contains("x-server-secret"));
    assert!(!seen[0].contains("from-the-mod"), "the mod's secret is only sent as a header");
    let body: serde_json::Value = serde_json::from_str(post_body).unwrap();
    assert_eq!(body["sub_action"], "CREATE_LISTING");
    assert_eq!(body["price_factor"], 1.1);
    assert_eq!(body["duration_hours"], 72);
    assert_eq!(body["sale_mode"], "FIXED");
    assert!(body.get("create_action_id").is_none());
    let vehicle = &body["vehicle"];
    assert_eq!(vehicle["xml_filename"], "$moddir$FS25_ExampleTractor/tractor.xml");
    assert_eq!(vehicle["mod_name"], "FS25_ExampleTractor");
    assert_eq!(vehicle["age_months"], 26);
    assert_eq!(vehicle["configurations"][0]["save_id"], "3");
    assert!(vehicle["configurations"][0].get("color").is_none());
    assert_eq!(vehicle["configurations"][1]["color"], serde_json::json!([0.62, 0.05, 0.04]));
    assert_eq!(vehicle["configurations"][1]["material_template"], "");

    assert!(seen[1].starts_with(&format!(
        "GET /api/v1/vehicles/state?server_id={IDENTITY_SERVER_ID} "
    )));
    assert!(seen[1].to_ascii_lowercase().contains("x-server-secret: issued-1"));
    assert!(seen[1].to_ascii_lowercase().contains("if-none-match: \"v1\""));
    assert!(seen[3].starts_with("GET /api/v1/vehicles/state "));
    // The board alone carries the remembered savegame identity of the last successful request (#14).
    let board_only = seen[3].to_ascii_lowercase();
    assert!(board_only.contains(&format!("x-server-id: {IDENTITY_SERVER_ID}")));
    assert_eq!(board_only.matches("x-server-secret").count(), 1);
}

#[test]
fn stage3_end_to_end_vehicle_bridge_with_mock_backend() {
    let (test_dir, config_file, bridge_root) = identity_test_profile(
        "vehicles",
        AppConfig {
            backend_mode: BackendMode::Mock,
            ..AppConfig::default()
        },
    );
    let read_dir = bridge_root.join("read");
    let mgr = BridgeManager::new(config_file, None);
    wait_for_heartbeat_flag(&bridge_root, "vehiclesSupported=\"true\"");
    thread::sleep(Duration::from_millis(800));
    assert!(
        read_sync_xmls(&read_dir, "gmVehicleSync_").is_empty(),
        "a board without identity must not reach the mod"
    );
    let snap = mgr.get_snapshot();
    assert!(snap.vehicles_supported);
    assert!(snap.open_vehicle_listings_count >= 2, "the UI still counts the board");

    let req = |attrs: &str| format!("    <request serverId=\"{IDENTITY_SERVER_ID}\" {attrs} />");

    // Registration through the vehicle board: secret and full board reach the mod.
    let xml = send_vehicle_xml(
        &bridge_root,
        1,
        &req("subAction=\"REQUEST_SYNC\" actionId=\"v_1\" serverSecret=\"\""),
        &|xml| xml.contains("<serverIdentity ") && xml.contains("<listings>"),
    );
    let issued = extract_attr(&xml, "serverIdentity", "serverSecret").expect("secret");
    assert!(xml.contains("<listing remoteId=\"mock_v_1\" createActionId=\"\" saleMode=\"FIXED\" sellerServerId=\"\""));
    assert!(xml.contains("color=\"0.62 0.05 0.04\""));
    assert_eq!(mgr.get_snapshot().server_identity, "known");

    // The same secret also goes to the contract bridge until the mod sends it.
    clear_dir(&read_dir);
    let contract_xml = send_action_xml(
        &bridge_root,
        11,
        "subAction=\"REQUEST_SYNC\" actionId=\"c_1\" serverSecret=\"\"",
        &|xml| xml.contains("<serverIdentity "),
    );
    assert_eq!(extract_attr(&contract_xml, "serverIdentity", "serverSecret"), Some(issued.clone()));

    // CREATE_LISTING with nested vehicle and paint: accepted, own listing with unmasked ids.
    clear_dir(&read_dir);
    let xml = send_vehicle_xml(
        &bridge_root,
        2,
        &format!(
            "    <request subAction=\"CREATE_LISTING\" actionId=\"v_2\" serverId=\"{IDENTITY_SERVER_ID}\" serverSecret=\"{issued}\" farmId=\"1\" farmName=\"Hof Meier\" saleMode=\"FIXED\" priceFactor=\"1.10\" durationHours=\"72\">\n        <vehicle xmlFilename=\"data/vehicles/claas/axion/axion.xml\" group=\"DRIVABLES\" ageMonths=\"26\" operatingHours=\"412.5\" damage=\"0.18\" wear=\"0.31\">\n            <config name=\"baseColor\" saveId=\"4\" isActive=\"true\" isBought=\"true\" color=\"0.1 0.2 0.3\" materialTemplate=\"metallic\" />\n        </vehicle>\n    </request>"
        ),
        &|xml| xml.contains("actionId=\"v_2\""),
    );
    let res_start = xml.find("<result actionId=\"v_2\"").unwrap();
    assert!(xml[res_start..].starts_with("<result actionId=\"v_2\" subAction=\"CREATE_LISTING\" status=\"ACCEPTED\" reason=\"\""));
    assert!(xml.contains(&format!("createActionId=\"v_2\" saleMode=\"FIXED\" sellerServerId=\"{IDENTITY_SERVER_ID}\"")));
    assert!(xml.contains("color=\"0.1 0.2 0.3\" materialTemplate=\"metallic\""));
    assert!(!xml.contains("<serverIdentity"), "the mod sent the secret itself");

    // BUY_LISTING of a foreign listing: result with vehicle and a LISTING_BOUGHT settlement.
    clear_dir(&read_dir);
    let xml = send_vehicle_xml(
        &bridge_root,
        3,
        &req(&format!("subAction=\"BUY_LISTING\" actionId=\"v_3\" serverSecret=\"{issued}\" farmId=\"2\" remoteId=\"mock_v_1\"")),
        &|xml| xml.contains("actionId=\"v_3\""),
    );
    assert!(xml.contains("<result actionId=\"v_3\" subAction=\"BUY_LISTING\" status=\"ACCEPTED\""));
    assert!(xml.contains("type=\"LISTING_BOUGHT\""));
    assert!(xml.contains("buyActionId=\"v_3\""));
    let settlement_id = extract_attr(&xml, "settlement", "settlementId").unwrap();
    assert!(!xml.contains("<listing remoteId=\"mock_v_1\""));

    // ACK_SETTLEMENT; a REQUEST_SYNC afterwards always gets a full board, without the settlement.
    clear_dir(&read_dir);
    send_vehicle_xml(
        &bridge_root,
        4,
        &format!(
            "{}\n{}",
            req(&format!("subAction=\"ACK_SETTLEMENT\" actionId=\"v_4\" serverSecret=\"{issued}\" settlementIds=\"{settlement_id}\"")),
            req(&format!("subAction=\"REQUEST_SYNC\" actionId=\"v_5\" serverSecret=\"{issued}\""))
        ),
        &|xml| xml.matches("<listings>").count() >= 2,
    );
    let last = read_sync_xmls(&read_dir, "gmVehicleSync_").pop().unwrap();
    assert!(last.contains("<listings>"));
    assert!(!last.contains(&settlement_id));

    // Cancel by a create id that never arrived, and an oversized snapshot rejected locally.
    clear_dir(&read_dir);
    let configs: String = (0..70)
        .map(|i| format!("<config name=\"c{i}\" saveId=\"{i}\" isActive=\"true\" isBought=\"true\" />"))
        .collect();
    let xml = send_vehicle_xml(
        &bridge_root,
        5,
        &format!(
            "{}\n    <request subAction=\"CREATE_LISTING\" actionId=\"v_7\" serverId=\"{IDENTITY_SERVER_ID}\" serverSecret=\"{issued}\" farmId=\"1\" priceFactor=\"1.0\" durationHours=\"24\"><vehicle xmlFilename=\"data/x.xml\">{configs}</vehicle></request>",
            req(&format!("subAction=\"CANCEL_LISTING\" actionId=\"v_6\" serverSecret=\"{issued}\" createActionId=\"never_sent_1\""))
        ),
        &|xml| xml.contains("actionId=\"v_6\"") && xml.contains("actionId=\"v_7\""),
    );
    assert!(xml.contains("<result actionId=\"v_6\" subAction=\"CANCEL_LISTING\" status=\"ACCEPTED\" reason=\"NOT_CREATED\" remoteId=\"\" createActionId=\"never_sent_1\""));
    assert!(xml.contains("<result actionId=\"v_7\" subAction=\"CREATE_LISTING\" status=\"REJECTED\" reason=\"INVALID_REQUEST\" remoteId=\"\" createActionId=\"v_7\""));

    let logs = mgr.get_logs();
    assert!(logs.iter().any(|l| l.key.as_deref() == Some("logVehicleActionSent")));
    assert!(logs.iter().any(|l| l.key.as_deref() == Some("logVehicleActionRejected")));
    assert!(logs.iter().any(|l| l.key.as_deref() == Some("logVehicleSettlementSynced")));
    assert!(mgr
        .generate_diagnostic_report()
        .contains("vehicles_bridge: \"supported=true"));

    mgr.shutdown();
    assert!(read_sync_xmls(&read_dir, "gmVehicleSync_").is_empty());
    let _ = fs::remove_dir_all(&test_dir);
}

#[test]
fn stage3_vehicle_actions_without_backend_support_are_rejected_for_the_mod() {
    // The identity test backend speaks API 2.8.0: no vehicle market.
    let (url, requests, stop, handle) = spawn_identity_server("good-secret", true);
    let (test_dir, config_file, bridge_root) = identity_test_profile(
        "novehicles",
        AppConfig {
            backend_mode: BackendMode::Live,
            backend_url: url,
            ..AppConfig::default()
        },
    );
    let mgr = BridgeManager::new(config_file, None);
    wait_for_contracts(&bridge_root);
    let heartbeat = fs::read_to_string(bridge_root.join("globalMarketOnline.xml")).unwrap();
    assert!(!heartbeat.contains("vehiclesSupported"));

    let xml = send_vehicle_xml(
        &bridge_root,
        1,
        &format!(
            "    <request subAction=\"CREATE_LISTING\" actionId=\"n_1\" serverId=\"{IDENTITY_SERVER_ID}\" farmId=\"1\" priceFactor=\"1.0\" durationHours=\"24\"><vehicle xmlFilename=\"data/x.xml\" /></request>\n    <request subAction=\"REQUEST_SYNC\" actionId=\"n_2\" serverId=\"{IDENTITY_SERVER_ID}\" />"
        ),
        &|xml| xml.contains("actionId=\"n_1\""),
    );
    assert!(xml.contains("<result actionId=\"n_1\" subAction=\"CREATE_LISTING\" status=\"REJECTED\" reason=\"VEHICLES_NOT_SUPPORTED\""));
    assert!(!xml.contains("actionId=\"n_2\""), "silent actions are not answered");
    assert!(!xml.contains("<listings"), "must not clear the mod's board");
    assert!(!requests.lock().unwrap().iter().any(|r| r.contains("/vehicles/")));

    mgr.shutdown();
    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    let _ = fs::remove_dir_all(&test_dir);
}

#[test]
fn stage3_rejected_identity_on_vehicle_actions_is_passed_to_the_mod_and_recovers() {
    let (url, requests, stop, handle) = spawn_identity_server_with("good-secret", false, true);
    let (test_dir, config_file, bridge_root) = identity_test_profile(
        "vehreject",
        AppConfig {
            backend_mode: BackendMode::Live,
            backend_url: url,
            ..AppConfig::default()
        },
    );
    let read_dir = bridge_root.join("read");
    let mgr = BridgeManager::new(config_file, None);
    wait_for_heartbeat_flag(&bridge_root, "vehiclesSupported=\"true\"");

    // Rejected also for silent actions, as a results-only file.
    let xml = send_vehicle_xml(
        &bridge_root,
        1,
        &format!("    <request subAction=\"REQUEST_SYNC\" actionId=\"r_1\" serverId=\"{IDENTITY_SERVER_ID}\" serverSecret=\"wrong-secret\" />"),
        &|xml| xml.contains("actionId=\"r_1\""),
    );
    assert!(xml.contains("<result actionId=\"r_1\" subAction=\"REQUEST_SYNC\" status=\"REJECTED\" reason=\"SERVER_SECRET_INVALID\""));
    assert!(!xml.contains("<listings"), "must not clear the mod's board");
    assert_eq!(mgr.get_snapshot().server_identity, "rejected");
    assert!(mgr
        .get_logs()
        .iter()
        .any(|l| l.key.as_deref() == Some("logServerIdentityRejected")));

    // Board only while rejected, and nothing of it reaches the mod.
    clear_dir(&read_dir);
    thread::sleep(Duration::from_millis(1500));
    assert!(read_sync_xmls(&read_dir, "gmVehicleSync_").is_empty());
    {
        let log = requests.lock().unwrap();
        assert!(log.iter().any(|r| r == "GET /api/v1/vehicles/state []"));
        assert!(!log.iter().any(|r| r.contains("vehicles/state?server_id=")));
    }

    // The right secret recovers and is used for polling.
    let xml = send_vehicle_xml(
        &bridge_root,
        2,
        &format!("    <request subAction=\"REQUEST_SYNC\" actionId=\"r_2\" serverId=\"{IDENTITY_SERVER_ID}\" serverSecret=\"good-secret\" />"),
        &|xml| xml.contains("remoteId=\"v_9\""),
    );
    assert!(xml.contains("color=\"0.62 0.05 0.04\""));
    assert!(!xml.contains("serverIdentity"), "only issued secrets are passed on");
    assert_eq!(mgr.get_snapshot().server_identity, "known");
    let expected = format!("GET /api/v1/vehicles/state?server_id={IDENTITY_SERVER_ID} [good-secret]");
    for _ in 0..100 {
        if requests.lock().unwrap().contains(&expected) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(requests.lock().unwrap().contains(&expected));

    mgr.shutdown();
    stop.store(true, Ordering::SeqCst);
    handle.join().unwrap();
    let _ = fs::remove_dir_all(&test_dir);
}

#[test]
fn dev_token_goes_only_to_development_servers() {
    assert!(dev_token_allowed("https://dev.gm.bydora.de"));
    assert!(dev_token_allowed("https://DEV.gm.bydora.de/"));
    assert!(dev_token_allowed("http://127.0.0.1:8000"));
    assert!(dev_token_allowed("http://localhost:8000"));
    assert!(!dev_token_allowed("https://gm.bydora.de"));
    assert!(!dev_token_allowed("http://dev.gm.bydora.de"));
    assert!(!dev_token_allowed("https://dev.gm.bydora.de.evil.example"));
    assert!(!dev_token_allowed("https://evil.example"));
    assert!(!dev_token_allowed("https://user@dev.gm.bydora.de"));
    assert!(!dev_token_allowed(""));
}

/// `X-Dev-Token` of each request, in order (`None` = header missing).
type SeenDevTokens = Arc<Mutex<Vec<Option<String>>>>;

/// Serves `/api/v1/meta/status` like a 2.10 development server and records each `X-Dev-Token`.
fn spawn_dev_token_server(
    requests: usize,
) -> (std::net::SocketAddr, SeenDevTokens, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_srv = Arc::clone(&seen);
    let handle = thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(&mut stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut dev_token = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("x-dev-token") {
                        dev_token = Some(v.trim().to_string());
                    }
                }
            }
            drop(reader);
            let (status, body) = match dev_token.as_deref() {
                Some("dev_valid_test_token") => (
                    "200 OK",
                    "{\"online\":true,\"min_exe_version\":\"0.9.9\",\"api_version\":\"2.10.0\",\"market_name\":\"Global Market Official\",\"server_env\":\"development\"}",
                ),
                Some(_) => ("403 Forbidden", "{\"detail\":{\"code\":\"DEV_TOKEN_INVALID\"}}"),
                None => ("403 Forbidden", "{\"detail\":{\"code\":\"DEV_TOKEN_REQUIRED\"}}"),
            };
            seen_srv.lock().unwrap().push(dev_token);
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (addr, seen, handle)
}

#[test]
fn dev_token_header_and_rejections_from_a_development_server() {
    let (addr, seen, handle) = spawn_dev_token_server(3);
    let client = FastApiBackend::new(
        format!("http://{addr}"),
        None,
        "c84f92a17b0e3d41".to_string(),
    )
    .unwrap();

    let missing = client.check_status().unwrap_err();
    assert!(matches!(missing, ApiError::DevTokenRejected { missing: true }));

    client.set_dev_token(Some("dev_revoked_token".to_string()));
    let invalid = client.check_status().unwrap_err();
    assert!(matches!(invalid, ApiError::DevTokenRejected { missing: false }));

    client.set_dev_token(Some("  dev_valid_test_token  ".to_string()));
    let handshake = client.check_status().unwrap();
    assert_eq!(handshake.api_version, "2.10.0");
    assert_eq!(handshake.server_env.as_deref(), Some("development"));

    handle.join().unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            None,
            Some("dev_revoked_token".to_string()),
            Some("dev_valid_test_token".to_string()),
        ]
    );
}

#[test]
fn dev_token_survives_a_vault_reload_and_can_be_removed() {
    let dir = env::temp_dir().join(format!("fs25_gm_vault_devtoken_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let vault_file = TokenVault::vault_path_for_config(&dir.join("settings.json"));

    let mut vault = TokenVault::load_or_empty(vault_file.clone());
    assert!(!vault.has_dev_token());
    assert!(vault.set_dev_token(Some("dev with space")).is_err());
    assert!(vault.set_dev_token(Some("   ")).is_err());
    assert!(vault.set_dev_token(Some(&"x".repeat(129))).is_err());
    vault.set_dev_token(Some(" dev_abcDEF-123_xyz ")).unwrap();
    vault.save().unwrap();
    assert!(vault_file.exists());

    // Windows seals the vault with DPAPI; macOS and Linux rely on 0600 permissions.
    #[cfg(windows)]
    {
        let raw = fs::read(&vault_file).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("dev_abcDEF-123_xyz"));
    }

    let mut reloaded = TokenVault::load_or_empty(vault_file.clone());
    assert!(reloaded.has_dev_token());
    reloaded.set_dev_token(None).unwrap();
    reloaded.save().unwrap();
    assert!(!vault_file.exists(), "an empty vault removes its file");
    assert!(!TokenVault::load_or_empty(vault_file).has_dev_token());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn server_env_is_normalized() {
    assert_eq!(normalize_server_env(Some("development")).as_deref(), Some("development"));
    assert_eq!(normalize_server_env(Some(" DEV ")).as_deref(), Some("development"));
    assert_eq!(normalize_server_env(Some("production")).as_deref(), Some("production"));
    assert_eq!(normalize_server_env(Some("staging")), None);
    assert_eq!(normalize_server_env(None), None);
}

#[test]
fn client_blocked_is_classified() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut reader = BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                break;
            }
        }
        drop(reader);
        let body = "{\"detail\":{\"code\":\"CLIENT_BLOCKED\"}}";
        let resp = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
    });
    let client = FastApiBackend::new(
        format!("http://{addr}"),
        None,
        "c84f92a17b0e3d41".to_string(),
    )
    .unwrap();
    let err = client.check_status().unwrap_err();
    assert!(matches!(err, ApiError::ClientBlocked), "{err:?}");
    handle.join().unwrap();
}

#[test]
fn mod_info_map_is_read_and_cleaned() {
    let mod_map = parse_mod_info_map(
        "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"no\"?>\n<gmModInfo modName=\"FS25_GlobalMarket\" modVersion=\"0.11.0.43\" modHash=\"3f2a\" mapId=\"FS25_ExampleMap.ExampleMap\" mapModName=\"FS25_ExampleMap\" mapHash=\"8b1d07aa\" updatedAt=\"2026-11-27 14:05:12\" />",
    )
    .expect("mod map");
    assert_eq!(mod_map.map_id, "FS25_ExampleMap.ExampleMap");
    assert_eq!(mod_map.map_mod_name.as_deref(), Some("FS25_ExampleMap"));
    assert_eq!(mod_map.map_hash.as_deref(), Some("8b1d07aa"));

    let basegame = parse_mod_info_map(
        "<gmModInfo modName=\"FS25_GlobalMarket\" mapId=\"MapUS\" mapModName=\"\" mapHash=\"\" />",
    )
    .expect("basegame map");
    assert_eq!(basegame.map_id, "MapUS");
    assert_eq!(basegame.map_mod_name, None);
    assert_eq!(basegame.map_hash, None);

    // A value that cannot go into a header is dropped, not sent cut or escaped.
    let odd = parse_mod_info_map(
        "<gmModInfo mapId=\" MapEU \" mapModName=\"FS25_Kärnten\" mapHash=\"dev\" />",
    )
    .unwrap();
    assert_eq!(odd.map_id, "MapEU");
    assert_eq!(odd.map_mod_name, None);
    assert_eq!(odd.map_hash.as_deref(), Some("dev"));

    // Mod before v0.11.0.43, a foreign root element, a too long id, broken XML.
    assert_eq!(parse_mod_info_map("<gmModInfo modName=\"FS25_GlobalMarket\" modVersion=\"0.11.0.42\" />"), None);
    assert_eq!(parse_mod_info_map("<other mapId=\"MapUS\" />"), None);
    assert_eq!(parse_mod_info_map(&format!("<gmModInfo mapId=\"{}\" />", "x".repeat(129))), None);
    assert_eq!(parse_mod_info_map("<gmModInfo mapId=\"MapUS\""), None);
    assert_eq!(parse_mod_info_map(""), None);

    let dir = env::temp_dir().join(format!("fs25_gm_modinfo_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("modInfo.xml");
    assert_eq!(read_mod_info_raw(&file), None);
    fs::write(&file, "<gmModInfo mapId=\"MapUS\" />").unwrap();
    assert_eq!(read_mod_info_raw(&file).as_deref(), Some("<gmModInfo mapId=\"MapUS\" />"));
    fs::write(&file, "x".repeat(20_000)).unwrap();
    assert_eq!(read_mod_info_raw(&file), None, "an oversized file is ignored");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn content_not_supported_is_classified_with_its_label() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for body in [
            "{\"detail\":{\"code\":\"CONTENT_NOT_SUPPORTED\",\"label\":\" Example\\u0007 Map \"}}",
            "{\"detail\":{\"code\":\"CONTENT_NOT_SUPPORTED\"}}",
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(&mut stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            drop(reader);
            let resp = format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    let client = FastApiBackend::new(
        format!("http://{addr}"),
        None,
        "c84f92a17b0e3d41".to_string(),
    )
    .unwrap();

    let named = client.check_status().unwrap_err();
    match &named {
        ApiError::ContentNotSupported { label } => assert_eq!(label.as_deref(), Some("Example Map")),
        other => panic!("unexpected {other:?}"),
    }
    let unnamed = client.check_status().unwrap_err();
    assert!(matches!(unnamed, ApiError::ContentNotSupported { label: None }), "{unnamed:?}");
    handle.join().unwrap();

    assert_eq!(classify_outage(&named, None, 0), Some(OfflineReason::UnsupportedContent));
    assert_eq!(OfflineReason::UnsupportedContent.as_str(), "unsupportedContent");
}

/// Request line plus the `X-Map-*`, `X-Server-Id` and `X-Server-Secret` headers of one request.
#[derive(Debug, Default, Clone, PartialEq)]
struct SeenContentHeaders {
    request_line: String,
    map_id: Option<String>,
    map_mod: Option<String>,
    map_hash: Option<String>,
    server_id: Option<String>,
    server_secrets: Vec<String>,
}

#[test]
fn map_and_savegame_identity_go_with_every_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Arc<Mutex<Vec<SeenContentHeaders>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_srv = Arc::clone(&seen);
    let handle = thread::spawn(move || {
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(&mut stream);
            let mut entry = SeenContentHeaders::default();
            reader.read_line(&mut entry.request_line).unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    let v = v.trim().to_string();
                    match k.trim().to_ascii_lowercase().as_str() {
                        "x-map-id" => entry.map_id = Some(v),
                        "x-map-mod" => entry.map_mod = Some(v),
                        "x-map-hash" => entry.map_hash = Some(v),
                        "x-server-id" => entry.server_id = Some(v),
                        "x-server-secret" => entry.server_secrets.push(v),
                        _ => {}
                    }
                }
            }
            drop(reader);
            let body = if entry.request_line.contains("/vehicles/state") {
                "{\"updated_at\":1,\"listings\":[]}"
            } else {
                "{\"online\":true,\"min_exe_version\":\"0.9.9\",\"api_version\":\"2.12.0\",\"market_name\":\"Global Market Official\"}"
            };
            seen_srv.lock().unwrap().push(entry);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    let client = FastApiBackend::new(
        format!("http://{addr}"),
        None,
        "c84f92a17b0e3d41".to_string(),
    )
    .unwrap();

    // 1. Mod map, no savegame identity yet.
    client.set_loaded_map(LoadedMap::from_attributes("FS25_ExampleMap.ExampleMap", "FS25_ExampleMap", "8b1d07aa"));
    client.check_status().unwrap();
    // 2. Board request with the savegame's own identity.
    client
        .fetch_vehicles_state(Some(IDENTITY_SERVER_ID), Some(&secret("issued-1")))
        .unwrap();
    // 3. The handshake now carries the remembered identity.
    client.check_status().unwrap();
    // 4. Basegame map after a new map load: identity forgotten, no X-Map-Mod / X-Map-Hash.
    client.forget_savegame_identity();
    client.set_loaded_map(LoadedMap::from_attributes("MapUS", "", ""));
    client.check_status().unwrap();
    // 5. Mod before v0.11.0.43 and another market: no map headers, no identity.
    client.set_loaded_map(None);
    client.check_status().unwrap();
    handle.join().unwrap();

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 5);
    let mod_map = (
        Some("FS25_ExampleMap.ExampleMap".to_string()),
        Some("FS25_ExampleMap".to_string()),
        Some("8b1d07aa".to_string()),
    );
    for entry in &seen[..3] {
        assert_eq!((entry.map_id.clone(), entry.map_mod.clone(), entry.map_hash.clone()), mod_map, "{entry:?}");
    }
    assert_eq!(seen[0].server_id, None);
    assert!(seen[0].server_secrets.is_empty());
    assert!(seen[1].request_line.contains("/api/v1/vehicles/state"));
    assert_eq!(seen[1].server_id.as_deref(), Some(IDENTITY_SERVER_ID));
    assert_eq!(seen[1].server_secrets, vec!["issued-1".to_string()], "the board identity is sent once");
    assert!(seen[2].request_line.contains("/api/v1/meta/status"));
    assert_eq!(seen[2].server_id.as_deref(), Some(IDENTITY_SERVER_ID));
    assert_eq!(seen[2].server_secrets, vec!["issued-1".to_string()]);
    assert_eq!(seen[3].map_id.as_deref(), Some("MapUS"));
    assert_eq!((seen[3].map_mod.clone(), seen[3].map_hash.clone()), (None, None));
    assert_eq!(seen[3].server_id, None);
    assert!(seen[3].server_secrets.is_empty());
    assert_eq!(seen[4], SeenContentHeaders { request_line: seen[4].request_line.clone(), ..Default::default() });
}
