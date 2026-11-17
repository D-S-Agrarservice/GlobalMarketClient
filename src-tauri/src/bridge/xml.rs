//! Authoritative XML serialization & deserialization for the FS25 Lua <-> EXE bridge.
//!
//! Enforces all rules from `External_Interface_Specification_FS25_GlobalMarket_Mod2EXE.md`:
//! 1. Strict **UTF-8 without BOM** (`EF BB BF` is never emitted; stripped if ever present on read).
//! 2. **Atomic writes** via sibling `.staging/*.tmp` (outside `read/`, so Lua's `getFiles(read/)`
//!    never sees half-written temp files!) + `std::fs::rename` with Windows file-lock retry.
//! 3. Invariant dot-decimal floats (`0.92`) and safe 32-bit clamping (`0..=i32::MAX`) for GIANTS C++ `getInt`.
//! 4. Polymorphic `<gmDataSync action="...">` routing (`updateWhitelist`, `transaction`, `contractAction`, silent skip on unknown).

use crate::api::types::{
    ContractActionRequest, ContractStateResponse, ContractSubAction, MarketItemDto,
    TransactionRequest, TransactionType, WhitelistItemDto, WhitelistSyncRequest,
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

/// Separate generous retention cap for unread `gmContractSync_*.xml` files in `read/`.
/// Never shares pruning with `gmDataSync_*.xml` so spot-market polls cannot evict unread `actionResults` or `settlements`.
pub const MAX_RETAINED_CONTRACT_SYNC_FILES: usize = 32;

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
    ContractActions(Vec<ContractActionRequest>),
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
    write_heartbeat_xml_with_latest(
        staging_dir,
        online_xml_path,
        is_connected,
        exe_version,
        min_version,
        None,
        branding,
    )
}

/// Atomically writes `globalMarketOnline.xml` including an optional non-blocking `<latestVersion>`
/// ("nice-to-have" server update announcement alongside mandatory `<minVersion>`).
pub fn write_heartbeat_xml_with_latest(
    staging_dir: &Path,
    online_xml_path: &Path,
    is_connected: bool,
    exe_version: &str,
    min_version: &str,
    latest_version: Option<&str>,
    branding: Option<&HeartbeatBranding>,
) -> Result<(), String> {
    write_heartbeat_xml_with_capabilities(
        staging_dir,
        online_xml_path,
        is_connected,
        exe_version,
        min_version,
        latest_version,
        branding,
        false,
    )
}

/// Atomically writes `globalMarketOnline.xml` including `<latestVersion>`, 3-tier market branding,
/// and the Etappe 2 `contractsSupported="true|false"` capability attribute.
#[allow(clippy::too_many_arguments)]
pub fn write_heartbeat_xml_with_capabilities(
    staging_dir: &Path,
    online_xml_path: &Path,
    is_connected: bool,
    exe_version: &str,
    min_version: &str,
    latest_version: Option<&str>,
    branding: Option<&HeartbeatBranding>,
    contracts_supported: bool,
) -> Result<(), String> {
    let timestamp = now_unix_secs();
    let connected_str = if is_connected { "true" } else { "false" };
    let contracts_str = if is_connected && contracts_supported {
        "true"
    } else {
        "false"
    };
    let ver_escaped = escape(exe_version);
    let min_escaped = escape(if min_version.trim().is_empty() {
        "0.9.9"
    } else {
        min_version.trim()
    });
    let latest_line = latest_version
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| format!("    <latestVersion>{}</latestVersion>\n", escape(v)))
        .unwrap_or_default();

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
        "timestamp=\"{timestamp}\" isConnected=\"{connected_str}\" contractsSupported=\"{contracts_str}\" brandingTier=\"{}\" isPrivateMarket=\"{is_priv_str}\"",
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
{latest_line}</globalMarketOnline>\n"
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
    write_market_data_xml_with_branding(staging_dir, read_dir, items, None)
}

/// Atomically writes `read/gmDataSync_<timestamp>.xml` (`action="marketData"`, Spec Section 3.3)
/// with optional packet-level market branding attributes consumed by `GlobalMarket:readMarketSyncBranding`.
pub fn write_market_data_xml_with_branding(
    staging_dir: &Path,
    read_dir: &Path,
    items: &[MarketItemDto],
    branding: Option<&HeartbeatBranding>,
) -> Result<PathBuf, String> {
    let ts = now_unix_secs();
    let seq = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = if seq == 1 {
        format!("gmDataSync_{ts}.xml")
    } else {
        format!("gmDataSync_{ts}_{seq}.xml")
    };
    let target_path = read_dir.join(filename);

    let root_tag = match branding {
        Some(raw_branding) => {
            let effective = raw_branding.normalized();
            let is_priv_str = if effective.is_private_market {
                "true"
            } else {
                "false"
            };
            let mut root_attrs = format!(
                "action=\"marketData\" brandingTier=\"{}\" isPrivateMarket=\"{is_priv_str}\"",
                effective.branding_tier
            );
            if effective.is_private_market {
                if let Some(ref name) = effective.market_name {
                    root_attrs.push_str(&format!(" marketName=\"{}\"", escape(name)));
                }
            }
            // Always emit `logoFile` when packet branding is present so Lua's `readMarketSyncBranding`
            // clears any previously active custom logo when switching back to Tier 0 or Tier 2.
            let rel_logo = if effective.has_custom_logo {
                effective.logo_file.as_deref().unwrap_or("")
            } else {
                ""
            };
            root_attrs.push_str(&format!(" logoFile=\"{}\"", escape(rel_logo)));
            format!("<gmDataSync {root_attrs}>\n")
        }
        None => "<gmDataSync action=\"marketData\">\n".to_string(),
    };

    let mut xml = String::with_capacity(256 + items.len() * 140);
    xml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"no\"?>\n");
    xml.push_str(&root_tag);
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

/// Atomically writes `read/gmContractSync_<ts>_<seq>.xml` (`action="contractData"`, Spec Section 3.5).
/// Uses a dedicated filename prefix (`gmContractSync_`) so `prune_market_data_xmls` never evicts
/// unread `actionResults` or `settlements`, and zero-pads `<seq>` so Lua's `table.sort(files)`
/// always processes packets in strict chronological order.
pub fn write_contract_data_xml(
    staging_dir: &Path,
    read_dir: &Path,
    state: &ContractStateResponse,
) -> Result<PathBuf, String> {
    let ts = now_unix_secs();
    let seq = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = format!("gmContractSync_{ts}_{seq:06}.xml");
    let target_path = read_dir.join(filename);

    let estimated_cap = 256
        + state.contracts.len() * 240
        + state.direct_offers.len() * 240
        + state.action_results.len() * 180
        + state.settlements.len() * 200;
    let mut xml = String::with_capacity(estimated_cap);
    xml.push_str("<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"no\"?>\n");
    xml.push_str("<gmDataSync action=\"contractData\">\n");

    xml.push_str("    <contracts>\n");
    for c in &state.contracts {
        let rid = escape(c.remote_id.trim());
        let ft = escape(c.fill_type.trim());
        if rid.is_empty() || ft.is_empty() {
            continue;
        }
        let origin = escape(if c.origin.trim().is_empty() {
            "ONLINE"
        } else {
            c.origin.trim()
        });
        let is_npc_str = if c.is_npc { "true" } else { "false" };
        let issuer_sid = escape(c.issuer_server_id.trim());
        let issuer_fid = c.issuer_farm_id.clamp(0, i32::MAX);
        let issuer_name = escape(c.issuer_name.trim());
        let contractor_sid = escape(c.contractor_server_id.trim());
        let contractor_fid = c.contractor_farm_id.clamp(0, i32::MAX);
        let total_liters = c.total_liters.clamp(0, i32::MAX as i64);
        let delivered_liters = c.delivered_liters.clamp(0, i32::MAX as i64);
        let factor = if c.base_trend_factor.is_finite() && c.base_trend_factor > 0.0 {
            c.base_trend_factor
        } else {
            1.0
        };
        let bonus_pct = c.bonus_pct.clamp(0, 1_000);
        let rem_secs = c.remaining_seconds.clamp(0, i32::MAX as i64);
        let status = escape(if c.status.trim().is_empty() {
            "OPEN"
        } else {
            c.status.trim()
        });

        xml.push_str(&format!(
            "        <contract remoteId=\"{rid}\" origin=\"{origin}\" isNpc=\"{is_npc_str}\" issuerServerId=\"{issuer_sid}\" issuerFarmId=\"{issuer_fid}\" issuerName=\"{issuer_name}\" contractorServerId=\"{contractor_sid}\" contractorFarmId=\"{contractor_fid}\" fillType=\"{ft}\" totalLiters=\"{total_liters}\" deliveredLiters=\"{delivered_liters}\" baseTrendFactor=\"{factor:.4}\" bonusPct=\"{bonus_pct}\" remainingSeconds=\"{rem_secs}\" status=\"{status}\" />\n"
        ));
    }
    xml.push_str("    </contracts>\n");

    xml.push_str("    <directOffers>\n");
    for o in &state.direct_offers {
        let rid = escape(o.remote_id.trim());
        let ft = escape(o.fill_type.trim());
        if rid.is_empty() || ft.is_empty() {
            continue;
        }
        let origin = escape(if o.origin.trim().is_empty() {
            "ONLINE"
        } else {
            o.origin.trim()
        });
        let is_npc_str = if o.is_npc { "true" } else { "false" };
        let seller_sid = escape(o.seller_server_id.trim());
        let seller_fid = o.seller_farm_id.clamp(0, i32::MAX);
        let seller_name = escape(o.seller_name.trim());
        let total_liters = o.total_liters.clamp(0, i32::MAX as i64);
        let rem_liters = o.remaining_liters.clamp(0, i32::MAX as i64);
        let delta_pct = o.price_delta_pct.clamp(-100, 1_000);
        let allow_partial_str = if o.allow_partial { "true" } else { "false" };
        let rem_secs = o.remaining_seconds.clamp(0, i32::MAX as i64);
        let was_bale_str = if o.was_bale { "true" } else { "false" };
        let was_pallet_str = if o.was_pallet { "true" } else { "false" };
        let status = escape(if o.status.trim().is_empty() {
            "OPEN"
        } else {
            o.status.trim()
        });

        xml.push_str(&format!(
            "        <offer remoteId=\"{rid}\" origin=\"{origin}\" isNpc=\"{is_npc_str}\" sellerServerId=\"{seller_sid}\" sellerFarmId=\"{seller_fid}\" sellerName=\"{seller_name}\" fillType=\"{ft}\" totalLiters=\"{total_liters}\" remainingLiters=\"{rem_liters}\" priceDeltaPct=\"{delta_pct}\" allowPartial=\"{allow_partial_str}\" remainingSeconds=\"{rem_secs}\" wasBale=\"{was_bale_str}\" wasPallet=\"{was_pallet_str}\" status=\"{status}\" />\n"
        ));
    }
    xml.push_str("    </directOffers>\n");

    xml.push_str("    <actionResults>\n");
    for r in &state.action_results {
        let aid = escape(r.action_id.trim());
        if aid.is_empty() {
            continue;
        }
        let sub = escape(r.sub_action.trim());
        let status = escape(if r.status.trim().is_empty() {
            "REJECTED"
        } else {
            r.status.trim()
        });
        let reason = escape(&r.reason);
        let rid = escape(r.remote_id.trim());
        let granted = r.granted_liters.clamp(0, i32::MAX as i64);
        let factor = if r.base_trend_factor.is_finite() && r.base_trend_factor > 0.0 {
            r.base_trend_factor
        } else {
            1.0
        };
        let bonus = r.bonus_pct.clamp(0, 1_000);
        let rem_secs = r.remaining_seconds.clamp(0, i32::MAX as i64);

        xml.push_str(&format!(
            "        <result actionId=\"{aid}\" subAction=\"{sub}\" status=\"{status}\" reason=\"{reason}\" remoteId=\"{rid}\" grantedLiters=\"{granted}\" baseTrendFactor=\"{factor:.4}\" bonusPct=\"{bonus}\" remainingSeconds=\"{rem_secs}\" />\n"
        ));
    }
    xml.push_str("    </actionResults>\n");

    xml.push_str("    <settlements>\n");
    for s in &state.settlements {
        let sid = escape(s.settlement_id.trim());
        if sid.is_empty() {
            continue;
        }
        let stype = escape(s.settlement_type.trim());
        let target_sid = escape(s.target_server_id.trim());
        let target_fid = s.target_farm_id.clamp(0, i32::MAX);
        let rid = escape(s.remote_id.trim());
        let ft = escape(s.fill_type.trim());
        let liters = s.liters.clamp(0, i32::MAX as i64);
        let missing = s.missing_liters.clamp(0, i32::MAX as i64);
        let factor = if s.base_trend_factor.is_finite() && s.base_trend_factor > 0.0 {
            s.base_trend_factor
        } else {
            1.0
        };
        let delta = s.price_delta_pct.clamp(-100, 1_000);
        let label = escape(&s.source_label);

        xml.push_str(&format!(
            "        <settlement settlementId=\"{sid}\" type=\"{stype}\" targetServerId=\"{target_sid}\" targetFarmId=\"{target_fid}\" remoteId=\"{rid}\" fillType=\"{ft}\" liters=\"{liters}\" missingLiters=\"{missing}\" baseTrendFactor=\"{factor:.4}\" priceDeltaPct=\"{delta}\" sourceLabel=\"{label}\" />\n"
        ));
    }
    xml.push_str("    </settlements>\n");
    xml.push_str("</gmDataSync>\n");

    atomic_write_utf8_no_bom(staging_dir, &target_path, &xml)?;
    prune_contract_sync_xmls(read_dir, MAX_RETAINED_CONTRACT_SYNC_FILES);
    Ok(target_path)
}

/// Returns `true` when at least one unread `gmContractSync_*.xml` file is present in `read_dir`.
pub fn has_unread_contract_sync_xml(read_dir: &Path) -> bool {
    fs::read_dir(read_dir)
        .ok()
        .map(|entries| {
            entries.flatten().any(|entry| {
                let path = entry.path();
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|name| parse_prefixed_sync_sort_key(name, "gmContractSync_"))
                        .is_some()
            })
        })
        .unwrap_or(false)
}

/// Prunes `gmDataSync_*.xml` snapshot files in `read_dir` so at most `keep_newest` remain.
/// Passing `keep_newest == 0` removes all `gmDataSync_*.xml` files (used on startup & shutdown).
pub fn prune_market_data_xmls(read_dir: &Path, keep_newest: usize) {
    prune_prefixed_sync_xmls(read_dir, "gmDataSync_", keep_newest);
}

/// Prunes `gmContractSync_*.xml` files in `read_dir` independently of spot-market `gmDataSync_*.xml` files.
pub fn prune_contract_sync_xmls(read_dir: &Path, keep_newest: usize) {
    prune_prefixed_sync_xmls(read_dir, "gmContractSync_", keep_newest);
}

fn prune_prefixed_sync_xmls(read_dir: &Path, prefix: &str, keep_newest: usize) {
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
            let key = parse_prefixed_sync_sort_key(name, prefix)?;
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

/// Removes unread `read/gmDataSync_*.xml` + `read/gmContractSync_*.xml` snapshots and any leftover
/// `.staging/*.tmp` files while keeping `globalMarketOnline.xml` intact (used during live market/token
/// restarts so Dedicated Server Lua polling never sees a transient missing heartbeat file).
pub fn cleanup_bridge_transfer_files(read_dir: &Path, staging_dir: &Path) {
    prune_market_data_xmls(read_dir, 0);
    prune_contract_sync_xmls(read_dir, 0);
    if let Ok(entries) = fs::read_dir(staging_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

/// Removes `globalMarketOnline.xml`, all unread `read/gmDataSync_*.xml` & `read/gmContractSync_*.xml` snapshots,
/// and any leftover `.staging/*.tmp` files when stopping or exiting the bridge.
pub fn cleanup_bridge_runtime_files(
    online_xml_path: &Path,
    read_dir: &Path,
    staging_dir: &Path,
) {
    remove_heartbeat_xml(online_xml_path);
    cleanup_bridge_transfer_files(read_dir, staging_dir);
}

fn parse_prefixed_sync_sort_key(filename: &str, prefix: &str) -> Option<(u64, u64)> {
    let stem = filename
        .strip_prefix(prefix)
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
    let mut contract_actions = Vec::new();
    let mut entry_idx = 0_usize;
    let mut request_idx = 0_usize;

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
                    "request" if action.as_deref() == Some("contractAction") => {
                        let mut sub_action: Option<ContractSubAction> = None;
                        let mut action_id = String::new();
                        let mut server_id = String::new();
                        let mut farm_id: Option<i32> = None;
                        let mut farm_name: Option<String> = None;
                        let mut remote_id: Option<String> = None;
                        let mut fill_type: Option<String> = None;
                        let mut liters: Option<i64> = None;
                        let mut base_trend_factor: Option<f32> = None;
                        let mut bonus_pct: Option<i32> = None;
                        let mut price_delta_pct: Option<i32> = None;
                        let mut allow_partial: Option<bool> = None;
                        let mut duration_hours: Option<i32> = None;
                        let mut was_bale: Option<bool> = None;
                        let mut was_pallet: Option<bool> = None;
                        let mut settlement_ids: Option<String> = None;

                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            let val = attr
                                .unescape_value()
                                .map(|v| v.into_owned())
                                .unwrap_or_default();
                            let trimmed = val.trim();
                            match key {
                                "subAction" => sub_action = ContractSubAction::from_xml_str(trimmed),
                                "actionId" => action_id = trimmed.to_string(),
                                "serverId" => server_id = trimmed.to_string(),
                                "farmId" => {
                                    farm_id = trimmed
                                        .parse::<f64>()
                                        .ok()
                                        .map(|n| n.round() as i32)
                                        .filter(|&n| n > 0);
                                }
                                "farmName" if !trimmed.is_empty() => {
                                    farm_name = Some(trimmed.to_string());
                                }
                                "remoteId" if !trimmed.is_empty() => {
                                    remote_id = Some(trimmed.to_string());
                                }
                                "fillType" if !trimmed.is_empty() => {
                                    fill_type = Some(trimmed.to_ascii_uppercase());
                                }
                                "liters" => {
                                    liters = trimmed
                                        .replace(',', ".")
                                        .parse::<f64>()
                                        .ok()
                                        .map(|n| n.round() as i64)
                                        .filter(|&n| n > 0);
                                }
                                "baseTrendFactor" => {
                                    base_trend_factor = trimmed
                                        .replace(',', ".")
                                        .parse::<f32>()
                                        .ok()
                                        .filter(|f| f.is_finite() && *f > 0.0);
                                }
                                "bonusPct" => {
                                    bonus_pct = trimmed
                                        .replace(',', ".")
                                        .parse::<f64>()
                                        .ok()
                                        .map(|n| n.round() as i32);
                                }
                                "priceDeltaPct" => {
                                    price_delta_pct = trimmed
                                        .replace(',', ".")
                                        .parse::<f64>()
                                        .ok()
                                        .map(|n| n.round() as i32);
                                }
                                "allowPartial" => {
                                    allow_partial = parse_xml_bool_opt(trimmed);
                                }
                                "durationHours" => {
                                    duration_hours = trimmed
                                        .replace(',', ".")
                                        .parse::<f64>()
                                        .ok()
                                        .map(|n| n.round() as i32)
                                        .filter(|&n| n > 0);
                                }
                                "wasBale" => {
                                    was_bale = parse_xml_bool_opt(trimmed);
                                }
                                "wasPallet" => {
                                    was_pallet = parse_xml_bool_opt(trimmed);
                                }
                                "settlementIds" if !trimmed.is_empty() => {
                                    settlement_ids = Some(trimmed.to_string());
                                }
                                _ => {}
                            }
                        }

                        if let Some(sub) = sub_action {
                            if !server_id.is_empty() {
                                if action_id.is_empty() {
                                    action_id = if request_idx == 0 {
                                        format!("{server_id}_{file_stem_tx_id}")
                                    } else {
                                        format!("{server_id}_{file_stem_tx_id}_{request_idx}")
                                    };
                                }
                                request_idx += 1;
                                contract_actions.push(ContractActionRequest {
                                    sub_action: sub,
                                    action_id,
                                    server_id,
                                    farm_id,
                                    farm_name,
                                    remote_id,
                                    fill_type,
                                    liters,
                                    base_trend_factor,
                                    bonus_pct,
                                    price_delta_pct,
                                    allow_partial,
                                    duration_hours,
                                    was_bale,
                                    was_pallet,
                                    settlement_ids,
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
        Some("contractAction") => Ok(IncomingPacket::ContractActions(contract_actions)),
        Some(other) => Ok(IncomingPacket::IgnoredUnknownAction(other.to_string())),
        None => Err("XML does not contain a <gmDataSync action=\"...\"> root element.".to_string()),
    }
}

fn parse_xml_bool_opt(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
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
