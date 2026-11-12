//! Anonymized diagnostic report builder for DSA Discord support (Phase 3).
//! Masks local OS usernames/home paths and never exposes private market tokens.

use crate::bridge::coordinator::{BridgeConnectionState, BridgeSnapshot, LogEntry, LogLevel};
use crate::config::{AppConfig, BackendMode};
use std::fs;
use std::path::Path;

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

    let has_token = config
        .market_token
        .as_deref()
        .is_some_and(|t| !t.trim().is_empty());
    let token_state = if has_token {
        "configured (redacted)"
    } else {
        "none (official public market)"
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

    let mut last_err = snapshot
        .last_error
        .as_deref()
        .map(mask_user_paths)
        .unwrap_or_else(|| "none".to_string());
    if !cid_trimmed.is_empty() {
        last_err = last_err.replace(cid_trimmed, "<CLIENT>");
    }
    if let Some(ref tok) = config.market_token {
        let t = tok.trim();
        if !t.is_empty() {
            last_err = last_err.replace(t, "<REDACTED>");
        }
    }

    let gd_str = if snapshot.active_great_demands.is_empty() {
        "none".to_string()
    } else {
        snapshot.active_great_demands.join(", ")
    };

    let brand = crate::tray::format_brand_title(snapshot.game_edition.as_deref());
    let mut report = String::with_capacity(2048);
    report.push_str(&format!(
        "**{brand} – Diagnostic Report (D-S-Agrarservice)**\n```yaml\n"
    ));
    report.push_str(&format!(
        "app_version: \"{} (protocol {})\"\n",
        env!("CARGO_PKG_VERSION"),
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
        "ui_settings: \"lang={}, tier={}, tray={}, notifications={}\"\n",
        config.language,
        config.access_tier.as_str(),
        config.minimize_to_tray,
        config.notifications_enabled
    ));
    report.push_str(&format!("backend_mode: \"{mode_str}\"\n"));
    report.push_str(&format!(
        "backend_url: \"{}\"\n",
        mask_user_paths(&config.backend_url)
    ));
    report.push_str(&format!("market_name: \"{}\"\n", snapshot.market_name));
    report.push_str(&format!("market_token: \"{token_state}\"\n"));
    report.push_str(&format!("client_slot_prefix: \"{slot_prefix}\"\n"));
    report.push_str(&format!(
        "connection_state: \"{} (min_exe_version={})\"\n",
        format_state(snapshot.state),
        snapshot.min_exe_version
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
    report.push_str(&format!("active_great_demands: \"{gd_str}\"\n"));
    report.push_str(&format!("last_error: \"{last_err}\"\n"));
    report.push_str("recent_logs:\n");

    let tail_start = logs.len().saturating_sub(15);
    if logs.is_empty() {
        report.push_str("  - \"<no log entries>\"\n");
    } else {
        let cid = config.client_id.trim();
        for entry in &logs[tail_start..] {
            let ts = format_hms_utc(entry.timestamp_ms);
            let lvl = format_level(entry.level);
            let mut msg = mask_user_paths(&entry.message);
            if !cid.is_empty() {
                msg = msg.replace(cid, "<CLIENT>");
            }
            if let Some(ref tok) = config.market_token {
                let t = tok.trim();
                if !t.is_empty() {
                    msg = msg.replace(t, "<REDACTED>");
                }
            }
            let escaped = msg.replace('"', "'");
            report.push_str(&format!("  - \"[{ts}] [{lvl}] {escaped}\"\n"));
        }
    }

    report.push_str("```");
    report
}
