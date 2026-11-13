//! Authoritative XML serialization & deserialization for the FS25 Lua <-> EXE bridge.
//!
//! Enforces all rules from `External_Interface_Specification_FS25_GlobalMarket_Mod2EXE.md`:
//! 1. Strict **UTF-8 without BOM** (`EF BB BF` is never emitted; stripped if ever present on read).
//! 2. **Atomic writes** via sibling `.staging/*.tmp` (outside `read/`, so Lua's `getFiles(read/)`
//!    never sees half-written temp files!) + `std::fs::rename` with Windows file-lock retry.
//! 3. Invariant dot-decimal floats (`0.92`) and safe 32-bit clamping (`0..=i32::MAX`) for GIANTS C++ `getInt`.
//! 4. Polymorphic `<gmDataSync action="...">` routing (`updateWhitelist`, `transaction`, silent skip on unknown).

use crate::api::types::{
    MarketItemDto, TransactionRequest, TransactionType, WhitelistItemDto, WhitelistSyncRequest,
};
use quick_xml::escape::escape;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Maximum number of unread `gmDataSync_*.xml` full-state snapshots retained in `read/` while running.
/// Each file carries the complete market state, so older snapshots are redundant; keeping up to 3
/// prevents directory buildup while the game is closed without racing an active Lua file read.
pub const MAX_RETAINED_MARKET_SYNC_FILES: usize = 3;

/// Maximum UTF-8 byte & character length accepted by `GlobalMarketBranding.lua` (`MAX_MARKET_NAME_LEN = 48`).
pub const MAX_MARKET_NAME_BYTES: usize = 48;

/// Optional 3-tier market branding attributes written onto `<globalMarketOnline ...>` (`FS25_GlobalMarket v0.9.9.22+`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeartbeatBranding {
    /// Branding tier (`0` = Official Standard, `1` = Official + Logo, `2` = Private Market, `3` = Private + Logo).
    pub branding_tier: u8,
    pub is_private_market: bool,
    pub market_name: Option<String>,
    pub has_custom_logo: bool,
    pub logo_file: Option<String>,
}

impl HeartbeatBranding {
    /// Enforces strict Lua tier invariants (`GlobalMarketBranding.lua`) and sanitizes `marketName` / `logoFile`.
    pub fn normalized(&self) -> Self {
        let clean_name = self.market_name.as_deref().and_then(sanitize_market_name);
        let clean_logo = self.logo_file.as_deref().and_then(sanitize_logo_rel_path);
        let logo_ready = self.has_custom_logo && clean_logo.is_some();

        match self.branding_tier {
            1 if logo_ready => Self {
                branding_tier: 1,
                is_private_market: false,
                market_name: None,
                has_custom_logo: true,
                logo_file: clean_logo,
            },
            2 => Self {
                branding_tier: 2,
                is_private_market: true,
                market_name: clean_name,
                has_custom_logo: false,
                logo_file: None,
            },
            3 if logo_ready => Self {
                branding_tier: 3,
                is_private_market: true,
                market_name: clean_name,
                has_custom_logo: true,
                logo_file: clean_logo,
            },
            3 => Self {
                branding_tier: 2,
                is_private_market: true,
                market_name: clean_name,
                has_custom_logo: false,
                logo_file: None,
            },
            _ => Self::default(),
        }
    }
}

/// Sanitizes an incoming custom market name identically to `GlobalMarketBranding.sanitizeMarketName`
/// while truncating on a valid UTF-8 codepoint boundary (`<= 48` UTF-8 bytes) so Lua `string.sub(1, 48)`
/// can never split multi-byte characters (such as German umlauts).
pub fn sanitize_market_name(raw: &str) -> Option<String> {
    let without_ctrl: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut collapsed = without_ctrl
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        return None;
    }
    if collapsed.len() > MAX_MARKET_NAME_BYTES {
        let mut end = MAX_MARKET_NAME_BYTES;
        while end > 0 && !collapsed.is_char_boundary(end) {
            end -= 1;
        }
        collapsed.truncate(end);
        collapsed = collapsed.trim_end().to_string();
    }
    (!collapsed.is_empty()).then_some(collapsed)
}

/// Validates a relative `.dds` logo path inside `modSettings/FS<YY>_GlobalMarket/` identically to
/// `GlobalMarketBranding.sanitizeLogoRelPath` (`<file>.dds` or `logos/<file>.dds`, ASCII `[A-Za-z0-9_.-]`).
pub fn sanitize_logo_rel_path(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    let clean = normalized.trim();
    if clean.is_empty() || clean.contains("..") || clean.contains(':') {
        return None;
    }

    let file_part = clean.strip_prefix("logos/").unwrap_or(clean);
    if file_part.len() <= 4
        || !file_part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return None;
    }

    if !file_part.to_ascii_lowercase().ends_with(".dds") {
        return None;
    }

    Some(clean.to_string())
}

#[derive(Debug, Clone)]
pub enum IncomingPacket {
    UpdateWhitelist(WhitelistSyncRequest),
    Transactions(Vec<TransactionRequest>),
    IgnoredUnknownAction(String),
}

/// Compares SemVer (`MAJOR.MINOR.PATCH`) and 4-part (`MAJOR.MINOR.PATCH.BUILD`) version strings
/// identically to `GlobalMarketSync.lua` (`isVersionSufficient`), padding missing segments with `0`.
pub fn is_version_sufficient(current_version: &str, min_version: &str) -> bool {
    let cur = parse_version_parts(current_version);
    let min = parse_version_parts(min_version);
    if cur.is_empty() {
        return false;
    }
    for i in 0..4 {
        let c = cur.get(i).copied().unwrap_or(0);
        let m = min.get(i).copied().unwrap_or(0);
        if c > m {
            return true;
        }
        if c < m {
            return false;
        }
    }
    true
}

fn parse_version_parts(raw: &str) -> Vec<u32> {
    raw.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u32>().ok())
        .collect()
}

/// Atomically writes `globalMarketOnline.xml` (Spec Section 3.1 + 3-tier market branding attributes).
pub fn write_heartbeat_xml(
    staging_dir: &Path,
    online_xml_path: &Path,
    is_connected: bool,
    exe_version: &str,
    min_version: &str,
    branding: Option<&HeartbeatBranding>,
) -> Result<(), String> {
    let timestamp = now_unix_secs();
    let connected_str = if is_connected { "true" } else { "false" };
    let ver_escaped = escape(exe_version);
    let min_escaped = escape(if min_version.trim().is_empty() {
        "1.0.0"
    } else {
        min_version
    });

    let effective = if is_connected {
        branding.map(HeartbeatBranding::normalized).unwrap_or_default()
    } else {
        HeartbeatBranding::default()
    };

    let is_priv_str = if effective.is_private_market {
        "true"
    } else {
        "false"
    };
    let has_logo_str = if effective.has_custom_logo {
        "true"
    } else {
        "false"
    };

    let mut root_attrs = format!(
        "timestamp=\"{timestamp}\" isConnected=\"{connected_str}\" brandingTier=\"{}\" isPrivateMarket=\"{is_priv_str}\"",
        effective.branding_tier
    );

    if effective.is_private_market {
        if let Some(ref name) = effective.market_name {
            let name_escaped = escape(name);
            root_attrs.push_str(&format!(" marketName=\"{name_escaped}\""));
        }
    }

    root_attrs.push_str(&format!(" hasCustomLogo=\"{has_logo_str}\""));

    if effective.has_custom_logo {
        if let Some(ref rel_logo) = effective.logo_file {
            let logo_escaped = escape(rel_logo);
            root_attrs.push_str(&format!(" logoFile=\"{logo_escaped}\""));
        }
    }

    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"no\"?>\n\
<globalMarketOnline {root_attrs}>\n\
    <version>{ver_escaped}</version>\n\
    <minVersion>{min_escaped}</minVersion>\n\
</globalMarketOnline>\n"
    );

    atomic_write_utf8_no_bom(staging_dir, online_xml_path, &xml)
}

/// Removes `globalMarketOnline.xml` immediately on clean shutdown so FS25 switches to Offline mode without a 10s delay.
pub fn remove_heartbeat_xml(online_xml_path: &Path) {
    if !online_xml_path.exists() {
        return;
    }
    for attempt in 0..5 {
        if fs::remove_file(online_xml_path).is_ok() || !online_xml_path.exists() {
            return;
        }
        if attempt < 4 {
            thread::sleep(Duration::from_millis(30));
        }
    }
}

/// Atomically writes `read/gmDataSync_<timestamp>.xml` (`action="marketData"`, Spec Section 3.3).
pub fn write_market_data_xml(
    staging_dir: &Path,
    read_dir: &Path,
    items: &[MarketItemDto],
) -> Result<PathBuf, String> {
    let ts = now_unix_secs();
    let seq = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = if seq == 1 {
        format!("gmDataSync_{ts}.xml")
    } else {
        format!("gmDataSync_{ts}_{seq}.xml")
    };
    let target_path = read_dir.join(filename);

    let mut xml = String::with_capacity(256 + items.len() * 140);
    xml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"no\"?>\n");
    xml.push_str("<gmDataSync action=\"marketData\">\n");
    xml.push_str("    <fillTypes>\n");

    for item in items {
        let ft = escape(item.fill_type.trim());
        let clamped_level = item.fill_level.clamp(0, i32::MAX as i64);
        let trend = if item.trend.is_finite() && item.trend > 0.0 {
            item.trend
        } else {
            1.0
        };
        let gd_str = if item.great_demand { "true" } else { "false" };
        let status_str = item.status.as_xml_str();

        if item.great_demand {
            let bonus = item.great_demand_bonus.max(0.0);
            let remaining = item.great_demand_remaining.clamp(0, i32::MAX as i64);
            xml.push_str(&format!(
                "        <fillType fillType=\"{ft}\" fillLevel=\"{clamped_level}\" trend=\"{trend:.2}\" greatDemand=\"{gd_str}\" greatDemandBonus=\"{bonus:.2}\" greatDemandRemaining=\"{remaining}\" status=\"{status_str}\" />\n"
            ));
        } else {
            xml.push_str(&format!(
                "        <fillType fillType=\"{ft}\" fillLevel=\"{clamped_level}\" trend=\"{trend:.2}\" greatDemand=\"{gd_str}\" status=\"{status_str}\" />\n"
            ));
        }
    }

    xml.push_str("    </fillTypes>\n");
    xml.push_str("</gmDataSync>\n");

    atomic_write_utf8_no_bom(staging_dir, &target_path, &xml)?;
    prune_market_data_xmls(read_dir, MAX_RETAINED_MARKET_SYNC_FILES);
    Ok(target_path)
}

/// Prunes `gmDataSync_*.xml` snapshot files in `read_dir` so at most `keep_newest` remain.
/// Passing `keep_newest == 0` removes all `gmDataSync_*.xml` files (used on startup & shutdown).
pub fn prune_market_data_xmls(read_dir: &Path, keep_newest: usize) {
    let Ok(entries) = fs::read_dir(read_dir) else {
        return;
    };
    let mut sync_files: Vec<((u64, u64), PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?;
            let key = parse_sync_filename_sort_key(name)?;
            Some((key, path))
        })
        .collect();

    if sync_files.len() <= keep_newest {
        return;
    }

    sync_files.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let delete_count = sync_files.len() - keep_newest;
    for (_, old_path) in sync_files.into_iter().take(delete_count) {
        let _ = fs::remove_file(old_path);
    }
}

/// Removes `globalMarketOnline.xml`, all unread `read/gmDataSync_*.xml` snapshots,
/// and any leftover `.staging/*.tmp` files when stopping or exiting the bridge.
pub fn cleanup_bridge_runtime_files(
    online_xml_path: &Path,
    read_dir: &Path,
    staging_dir: &Path,
) {
    remove_heartbeat_xml(online_xml_path);
    prune_market_data_xmls(read_dir, 0);
    if let Ok(entries) = fs::read_dir(staging_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn parse_sync_filename_sort_key(filename: &str) -> Option<(u64, u64)> {
    let stem = filename
        .strip_prefix("gmDataSync_")
        .and_then(|s| s.strip_suffix(".xml").or_else(|| s.strip_suffix(".XML")))?;
    match stem.split_once('_') {
        Some((ts_str, seq_str)) => {
            let ts = ts_str.parse::<u64>().unwrap_or(0);
            let seq = seq_str.parse::<u64>().unwrap_or(0);
            Some((ts, seq))
        }
        None => {
            let ts = stem.parse::<u64>().unwrap_or(0);
            Some((ts, 1))
        }
    }
}

/// Parses an XML file written by Lua into `modSettings/FS25_GlobalMarket/write/`.
/// `file_stem_tx_id` is extracted from e.g. `transaction_1729512015_1.xml` -> `"1729512015_1"`.
pub fn parse_incoming_write_xml(
    raw_content: &str,
    file_stem_tx_id: &str,
) -> Result<IncomingPacket, String> {
    let cleaned = raw_content.trim_start_matches('\u{FEFF}');
    let mut reader = Reader::from_str(cleaned);
    reader.config_mut().trim_text(true);

    let mut action: Option<String> = None;
    let mut lang = "en".to_string();
    let mut whitelist_items = Vec::new();
    let mut transactions = Vec::new();
    let mut entry_idx = 0_usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let tag_bytes = e.name();
                let tag = std::str::from_utf8(tag_bytes.as_ref()).unwrap_or("");

                match tag {
                    "gmDataSync" => {
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            let val = attr
                                .unescape_value()
                                .map(|v| v.into_owned())
                                .unwrap_or_default();
                            match key {
                                "action" => action = Some(val),
                                "lang" if !val.trim().is_empty() => {
                                    lang = val.trim().to_ascii_lowercase()
                                }
                                _ => {}
                            }
                        }
                    }
                    "item" if action.as_deref() == Some("updateWhitelist") => {
                        let mut name = String::new();
                        let mut title = String::new();
                        let mut category = "PRODUCTS_PALLETS".to_string();
                        let mut item_type = "BULK".to_string();
                        let mut base_price = 0.0_f32;
                        let mut source = "BASEGAME".to_string();

                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            let val = attr
                                .unescape_value()
                                .map(|v| v.into_owned())
                                .unwrap_or_default();
                            match key {
                                "name" => name = val.trim().to_ascii_uppercase(),
                                "title" => title = val,
                                "category" if !val.trim().is_empty() => category = val,
                                "type" if !val.trim().is_empty() => item_type = val,
                                "basePrice" => {
                                    base_price = val.replace(',', ".").parse::<f32>().unwrap_or(0.0)
                                }
                                "source" if !val.trim().is_empty() => source = val,
                                _ => {}
                            }
                        }

                        if !name.is_empty() {
                            if title.trim().is_empty() {
                                title = name.clone();
                            }
                            whitelist_items.push(WhitelistItemDto {
                                name,
                                title,
                                category,
                                item_type,
                                base_price,
                                source,
                            });
                        }
                    }
                    "entry" if action.as_deref() == Some("transaction") => {
                        let mut fill_type = String::new();
                        let mut amount = 0_i64;
                        let mut tx_type: Option<TransactionType> = None;

                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            let val = attr
                                .unescape_value()
                                .map(|v| v.into_owned())
                                .unwrap_or_default();
                            match key {
                                "fillType" => fill_type = val.trim().to_ascii_uppercase(),
                                "amount" => {
                                    amount = val
                                        .trim()
                                        .parse::<f64>()
                                        .map(|n| n.round() as i64)
                                        .unwrap_or(0)
                                }
                                "type" => tx_type = TransactionType::from_xml_str(&val),
                                _ => {}
                            }
                        }

                        if !fill_type.is_empty() && amount > 0 {
                            if let Some(kind) = tx_type {
                                let tx_id = if entry_idx == 0 {
                                    file_stem_tx_id.to_string()
                                } else {
                                    format!("{file_stem_tx_id}_{entry_idx}")
                                };
                                entry_idx += 1;
                                transactions.push(TransactionRequest {
                                    tx_id,
                                    fill_type,
                                    amount,
                                    tx_type: kind,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("Invalid XML syntax: {e}")),
            _ => {}
        }
    }

    match action.as_deref() {
        Some("updateWhitelist") => Ok(IncomingPacket::UpdateWhitelist(WhitelistSyncRequest {
            lang,
            items: whitelist_items,
        })),
        Some("transaction") => Ok(IncomingPacket::Transactions(transactions)),
        Some(other) => Ok(IncomingPacket::IgnoredUnknownAction(other.to_string())),
        None => Err("XML does not contain a <gmDataSync action=\"...\"> root element.".to_string()),
    }
}

/// Writes pure UTF-8 bytes (strictly NO BOM) to a unique file inside `staging_dir`,
/// flushes the file handle completely, and atomically renames it to `target_path`
/// (with short retries in case GIANTS Engine briefly locks `globalMarketOnline.xml` on Windows).
fn atomic_write_utf8_no_bom(
    staging_dir: &Path,
    target_path: &Path,
    xml_content: &str,
) -> Result<(), String> {
    fs::create_dir_all(staging_dir)
        .map_err(|e| format!("Failed to create staging directory: {e}"))?;
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create target directory: {e}"))?;
    }

    let seq = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_path = staging_dir.join(format!("stage_{}_{}.tmp", std::process::id(), seq));

    let bytes = xml_content.trim_start_matches('\u{FEFF}').as_bytes();
    debug_assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]));

    {
        let mut file = File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temporary XML file: {e}"))?;
        file.write_all(bytes)
            .map_err(|e| format!("Failed to write temporary XML file: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Failed to flush temporary XML file: {e}"))?;
    }

    let mut last_err = String::new();
    for attempt in 0..5 {
        match fs::rename(&tmp_path, target_path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e.to_string();
                if attempt < 4 {
                    thread::sleep(Duration::from_millis(40));
                }
            }
        }
    }

    let _ = fs::remove_file(&tmp_path);
    Err(format!(
        "Atomic rename to '{}' failed: {last_err}",
        target_path.display()
    ))
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
