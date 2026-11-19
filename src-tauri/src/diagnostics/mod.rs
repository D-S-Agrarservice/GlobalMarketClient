//! Anonymized diagnostic report builder for DSA Discord support (Phase 3).
//! Masks local OS usernames/home paths and never exposes private market tokens.

use crate::bridge::coordinator::{BridgeConnectionState, BridgeSnapshot, LogEntry, LogLevel};
use crate::config::{AccessTier, AppConfig, BackendMode};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Number of trailing log lines in the standard report.
const REPORT_LOG_LINES_STANDARD: usize = 15;
/// Number of trailing log lines when Extended or Developer mode is active.
const REPORT_LOG_LINES_EXTENDED: usize = 50;
/// Number of most recent warnings/errors listed separately in Extended or Developer mode.
const REPORT_RECENT_ERRORS: usize = 10;

/// Scrubs user-identifying home directory prefixes and usernames from paths and log lines.
pub fn mask_user_paths(input: &str) -> String {
    let mut out = input.to_string();

    if let Ok(profile) = std::env::var("USERPROFILE") {
        let trimmed = profile.trim();
        if !trimmed.is_empty() {
            out = replace_case_insensitive(&out, trimmed, "%USERPROFILE%");
            let slash_variant = trimmed.replace('\\', "/");
            out = replace_case_insensitive(&out, &slash_variant, "%USERPROFILE%");
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            out = out.replace(trimmed, "~");
        }
    }

    for var in ["USERNAME", "USER"] {
        if let Ok(user) = std::env::var(var) {
            let u = user.trim();
            if u.len() >= 2 {
                out = replace_case_insensitive(&out, &format!("\\Users\\{u}"), "\\Users\\<USER>");
                out = replace_case_insensitive(&out, &format!("/Users/{u}"), "/Users/<USER>");
                out = out.replace(&format!("/home/{u}"), "/home/<USER>");
            }
        }
    }

    out
}

fn replace_case_insensitive(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let lower_hay = haystack.to_lowercase();
    let lower_needle = needle.to_lowercase();

    // Only perform ASCII/equal-byte-length safe slicing; fall back to exact replace if unicode folding changes byte length.
    if lower_hay.len() != haystack.len() || lower_needle.len() != needle.len() {
        return haystack.replace(needle, replacement);
    }

    let mut result = String::with_capacity(haystack.len());
    let mut last_idx = 0;
    while let Some(rel_pos) = lower_hay[last_idx..].find(&lower_needle) {
        let abs_pos = last_idx + rel_pos;
        result.push_str(&haystack[last_idx..abs_pos]);
        result.push_str(replacement);
        last_idx = abs_pos + needle.len();
    }
    result.push_str(&haystack[last_idx..]);
    result
}

fn count_xml_files(dir: &Path) -> usize {
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

fn format_state(state: BridgeConnectionState) -> &'static str {
    match state {
        BridgeConnectionState::PathMissing => "PATH_MISSING",
        BridgeConnectionState::Disconnected => "DISCONNECTED",
        BridgeConnectionState::Connecting => "CONNECTING",
        BridgeConnectionState::Online => "ONLINE",
        BridgeConnectionState::BackendOffline => "BACKEND_OFFLINE",
        BridgeConnectionState::Outdated => "OUTDATED",
    }
}

fn format_level(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "INFO",
        LogLevel::Success => "OK  ",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERR ",
    }
}

fn format_hms_utc(timestamp_ms: u64) -> String {
    let total_secs = timestamp_ms / 1000;
    let millis = timestamp_ms % 1000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = (total_secs / 3600) % 24;
    format!("{hours:02}:{mins:02}:{secs:02}.{millis:03}Z")
}

/// Generates a Discord-ready Markdown diagnostic block with all sensitive values redacted.
pub fn build_diagnostic_report(
    snapshot: &BridgeSnapshot,
    config: &AppConfig,
    logs: &[LogEntry],
) -> String {
    build_diagnostic_report_with_secrets(snapshot, config, logs, &[])
}

/// Generates a Discord-ready Markdown diagnostic block while scrubbing both legacy `config.market_token`
/// and all secrets held inside the OS-protected `TokenVault`.
pub fn build_diagnostic_report_with_secrets(
    snapshot: &BridgeSnapshot,
    config: &AppConfig,
    logs: &[LogEntry],
    vault_secrets: &[String],
) -> String {
    let profile_masked = snapshot
        .profile_path
        .as_deref()
        .map(mask_user_paths)
        .unwrap_or_else(|| "<not set>".to_string());

    let bridge_masked = snapshot
        .bridge_path
        .as_deref()
        .map(mask_user_paths)
        .unwrap_or_else(|| "<not set>".to_string());

    let (online_xml_exists, read_count, write_count) =
        if let Some(ref bridge_str) = snapshot.bridge_path {
            let root = Path::new(bridge_str);
            let online = root.join("globalMarketOnline.xml").is_file();
            let r_cnt = count_xml_files(&root.join("read"));
            let w_cnt = count_xml_files(&root.join("write"));
            (online.to_string(), r_cnt.to_string(), w_cnt.to_string())
        } else {
            ("n/a".to_string(), "n/a".to_string(), "n/a".to_string())
        };

    let mode_str = match config.backend_mode {
        BackendMode::Mock => "mock (local simulation)",
        BackendMode::Live => "live (FastAPI)",
    };

    let has_token = config.active_token_id.is_some()
        || snapshot.active_token_id.is_some()
        || config
            .market_token
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty());
    let saved_cnt = snapshot.saved_tokens.len();
    let token_state = if has_token {
        if saved_cnt > 0 {
            format!("configured (redacted, vault_profiles={saved_cnt})")
        } else {
            "configured (redacted)".to_string()
        }
    } else if saved_cnt > 0 {
        format!("none (official public market, vault_profiles={saved_cnt})")
    } else {
        "none (official public market)".to_string()
    };

    let cid_trimmed = config.client_id.trim();
    let slot_prefix = if cid_trimmed.len() >= 6 {
        let head: String = cid_trimmed.chars().take(6).collect();
        format!("{head}…")
    } else if cid_trimmed.is_empty() {
        "<none>".to_string()
    } else {
        "<short>".to_string()
    };

    let cid_full = config.client_id.trim().to_string();
    let scrub = |input: &str| -> String {
        let mut out = mask_user_paths(input);
        if !cid_full.is_empty() {
            out = out.replace(&cid_full, "<CLIENT>");
        }
        if let Some(ref tok) = config.market_token {
            let t = tok.trim();
            if !t.is_empty() {
                out = out.replace(t, "<REDACTED>");
            }
        }
        for sec in vault_secrets {
            let t = sec.trim();
            if !t.is_empty() {
                out = out.replace(t, "<REDACTED>");
            }
        }
        out
    };

    let last_err = snapshot
        .last_error
        .as_deref()
        .map(&scrub)
        .unwrap_or_else(|| "none".to_string());

    let is_extended = matches!(
        config.access_tier,
        AccessTier::Extended | AccessTier::Developer
    );

    let gd_str = if snapshot.active_great_demands.is_empty() {
        "none".to_string()
    } else {
        snapshot.active_great_demands.join(", ")
    };

    let brand = crate::tray::format_brand_title(snapshot.game_edition.as_deref());
    let app_ver_label = if crate::IS_BETA_BUILD {
        format!("{}-beta", env!("CARGO_PKG_VERSION"))
    } else {
        env!("CARGO_PKG_VERSION").to_string()
    };
    let mut report = String::with_capacity(2048);
    report.push_str(&format!(
        "**{brand} – Diagnostic Report (D-S-Agrarservice)**\n```yaml\n"
    ));
    report.push_str(&format!(
        "app_version: \"{app_ver_label} (protocol {})\"\n",
        snapshot.exe_version
    ));
    report.push_str(&format!(
        "game_edition: \"{}\"\n",
        snapshot.game_edition.as_deref().unwrap_or("<not detected>")
    ));
    report.push_str(&format!(
        "os_platform: \"{} / {}\"\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    report.push_str(&format!(
        "ui_settings: \"lang={}, tier={}, tray={}, notifications={}, auto_connect={}\"\n",
        config.language,
        config.access_tier.as_str(),
        config.minimize_to_tray,
        config.notifications_enabled,
        config.auto_connect_on_launch
    ));
    report.push_str(&format!("backend_mode: \"{mode_str}\"\n"));
    report.push_str(&format!(
        "backend_url: \"{}\"\n",
        mask_user_paths(&config.backend_url)
    ));
    report.push_str(&format!("market_name: \"{}\"\n", snapshot.market_name));
    report.push_str(&format!(
        "branding: \"tier={}, private={}, logo_dds={}\"\n",
        snapshot.branding_tier,
        snapshot.is_private_market,
        snapshot.market_logo_file.as_deref().unwrap_or("none")
    ));
    report.push_str(&format!("market_token: \"{token_state}\"\n"));
    report.push_str(&format!("client_slot_prefix: \"{slot_prefix}\"\n"));
    report.push_str(&format!(
        "connection_state: \"{} (min_exe_version={}, latest_exe_version={})\"\n",
        format_state(snapshot.state),
        snapshot.min_exe_version,
        snapshot.latest_exe_version.as_deref().unwrap_or("none")
    ));
    report.push_str(&format!("fs_profile_dir: \"{profile_masked}\"\n"));
    report.push_str(&format!("bridge_dir: \"{bridge_masked}\"\n"));
    report.push_str(&format!(
        "bridge_files: \"online_xml={online_xml_exists}, lifebit_seq={}, read_queue={read_count}, write_queue={write_count}\"\n",
        snapshot.heartbeat_seq
    ));
    report.push_str(&format!(
        "whitelist_items: \"approved={}, pending={}, rejected={}\"\n",
        snapshot.approved_count, snapshot.pending_count, snapshot.rejected_count
    ));
    report.push_str(&format!(
        "contracts_bridge: \"supported={}, open_contracts={}, open_offers={}\"\n",
        snapshot.contracts_supported, snapshot.open_contracts_count, snapshot.open_offers_count
    ));
    report.push_str(&format!("active_great_demands: \"{gd_str}\"\n"));
    report.push_str(&format!("last_error: \"{last_err}\"\n"));

    if is_extended {
        let fmt_unix = |secs: Option<u64>| -> String {
            secs.map(|s| format_hms_utc(s * 1000))
                .unwrap_or_else(|| "none".to_string())
        };
        let remaining = match snapshot.extended_expires_unix {
            Some(deadline) => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                format!("{} min", deadline.saturating_sub(now).div_ceil(60))
            }
            None => "n/a".to_string(),
        };
        report.push_str(&format!(
            "api_version: \"{}\"\n",
            snapshot.api_version.as_deref().unwrap_or("unknown")
        ));
        report.push_str(&format!(
            "timing: \"last_sync={}, last_heartbeat={}, extended_remaining={remaining}\"\n",
            fmt_unix(snapshot.last_sync_unix),
            fmt_unix(snapshot.last_heartbeat_unix)
        ));

        report.push_str("recent_errors:\n");
        let errors: Vec<&LogEntry> = logs
            .iter()
            .filter(|e| matches!(e.level, LogLevel::Warn | LogLevel::Error))
            .collect();
        if errors.is_empty() {
            report.push_str("  - \"<none>\"\n");
        } else {
            let start = errors.len().saturating_sub(REPORT_RECENT_ERRORS);
            for entry in &errors[start..] {
                report.push_str(&format_log_line(entry, &scrub));
            }
        }
    }

    report.push_str("recent_logs:\n");

    let line_count = if is_extended {
        REPORT_LOG_LINES_EXTENDED
    } else {
        REPORT_LOG_LINES_STANDARD
    };
    let tail_start = logs.len().saturating_sub(line_count);
    if logs.is_empty() {
        report.push_str("  - \"<no log entries>\"\n");
    } else {
        for entry in &logs[tail_start..] {
            report.push_str(&format_log_line(entry, &scrub));
        }
    }

    report.push_str("```");
    report
}

/// Formats one scrubbed log entry as a YAML list item.
fn format_log_line(entry: &LogEntry, scrub: &dyn Fn(&str) -> String) -> String {
    let ts = format_hms_utc(entry.timestamp_ms);
    let lvl = format_level(entry.level);
    let escaped = scrub(&entry.message).replace('"', "'");
    format!("  - \"[{ts}] [{lvl}] {escaped}\"\n")
}
