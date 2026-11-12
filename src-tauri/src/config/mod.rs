//! Persistent user configuration (FS25 profile path, language, notifications, access tier, client ID, optional market token).

use serde::{Deserialize, Serialize};
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static CLIENT_ID_NONCE: AtomicU64 = AtomicU64::new(1);

/// Default production backend endpoint used when operating in `AccessTier::User` / `AccessTier::Extended`.
pub const DEFAULT_OFFICIAL_BACKEND_URL: &str = "https://gm.bydora.de";

/// Official development / staging backend endpoint selectable in `AccessTier::Developer`.
pub const OFFICIAL_DEV_BACKEND_URL: &str = "https://dev.gm.bydora.de";

/// Official dedicated CDN / image origin for Community & Logo keys (`market_logo_url`).
pub const OFFICIAL_LOGO_CDN_URL: &str = "https://img.gm.bydora.de";

const EXTENDED_TIER_DIGEST: (u64, u64) = (0xF2A3_2644_4335_EC05, 0xAAD2_5898_16B4_2471);
const DEVELOPER_TIER_DIGEST: (u64, u64) = (0x9DD1_6667_20FD_AED2, 0x1AA6_AF13_D46A_DEB2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendMode {
    /// Local in-memory simulation mode for testing the FS25 Lua mod offline/locally.
    Mock,
    /// Live connection to the Python / FastAPI backend.
    Live,
}

impl Default for BackendMode {
    fn default() -> Self {
        Self::Live
    }
}

/// Progressive UI & feature access tier unlocked via 5x version click + access key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessTier {
    /// Standard player mode: fixed official server URL, clean UI without developer controls.
    User,
    /// Extended support/tester mode: displays active server URL, API version, bridge path, and XML queue counters.
    Extended,
    /// Full developer & maintenance mode: unlocks Mock simulation tools and custom Backend Server URL configuration.
    Developer,
}

impl Default for AccessTier {
    fn default() -> Self {
        Self::User
    }
}

impl AccessTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Extended => "extended",
            Self::Developer => "developer",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    /// Persistent random per-installation identifier used to prefix outgoing `tx_id`s
    /// so concurrent players never collide on identical Unix-second filenames while
    /// preserving retry idempotency for the same local XML transaction file.
    pub client_id: String,
    /// Active UI & feature access tier (`user`, `extended`, `developer`).
    pub access_tier: AccessTier,
    /// Resolved FS25 profile directory (or custom parent folder containing `modSettings/FS25_GlobalMarket`).
    pub fs25_profile_path: Option<String>,
    /// UI language code (`de`, `en`, `fr`, `es`, `pl`). Fallback is always `en`.
    pub language: String,
    /// Minimize window to the system tray instead of quitting when clicking the window close button.
    pub minimize_to_tray: bool,
    /// Show OS desktop notifications (e.g. when a new global Great Demand starts).
    pub notifications_enabled: bool,
    /// Automatically scroll the Live Activity Log to the newest entry as events arrive.
    pub log_auto_scroll: bool,
    /// Active backend mode (`mock` while developing/testing locally, `live` for FastAPI server).
    pub backend_mode: BackendMode,
    /// Base URL of the FastAPI server (e.g. `https://api.example.com`).
    pub backend_url: String,
    /// Optional license/access token for private/community markets (empty = public global market).
    pub market_token: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            client_id: generate_client_id(),
            access_tier: AccessTier::User,
            fs25_profile_path: None,
            language: "de".to_string(),
            minimize_to_tray: true,
            notifications_enabled: true,
            log_auto_scroll: true,
            backend_mode: BackendMode::Live,
            backend_url: DEFAULT_OFFICIAL_BACKEND_URL.to_string(),
            market_token: None,
        }
    }
}

impl AppConfig {
    pub fn load_from_file(config_file: &Path) -> Self {
        if let Ok(raw) = fs::read_to_string(config_file) {
            if let Ok(mut parsed) = serde_json::from_str::<AppConfig>(&raw) {
                parsed.language = normalize_language(&parsed.language);
                let mut needs_save = false;

                if !is_valid_client_id(&parsed.client_id) {
                    parsed.client_id = generate_client_id();
                    needs_save = true;
                }

                // Extended (Support/Diagnostic) mode is strictly ephemeral and always resets on startup.
                // Only Developer mode persists across application restarts.
                if parsed.access_tier == AccessTier::Extended {
                    parsed.access_tier = AccessTier::User;
                    needs_save = true;
                }

                // Enforce standard live configuration whenever not in Developer mode.
                if parsed.access_tier != AccessTier::Developer {
                    if parsed.backend_mode != BackendMode::Live {
                        parsed.backend_mode = BackendMode::Live;
                        needs_save = true;
                    }
                    if parsed.backend_url != DEFAULT_OFFICIAL_BACKEND_URL {
                        parsed.backend_url = DEFAULT_OFFICIAL_BACKEND_URL.to_string();
                        needs_save = true;
                    }
                }

                if needs_save {
                    let _ = parsed.save_to_file(config_file);
                }
                return parsed;
            }
        }
        Self::default()
    }

    pub fn save_to_file(&self, config_file: &Path) -> Result<(), String> {
        if let Some(parent) = config_file.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {e}"))?;
        }
        // Never persist ephemeral `AccessTier::Extended` to disk; only `User` and `Developer` persist.
        let mut disk_copy = self.clone();
        if disk_copy.access_tier == AccessTier::Extended {
            disk_copy.access_tier = AccessTier::User;
        }
        let json = serde_json::to_string_pretty(&disk_copy)
            .map_err(|e| format!("Failed to serialize configuration: {e}"))?;

        let tmp_path: PathBuf = config_file.with_extension("json.tmp");
        fs::write(&tmp_path, json.as_bytes())
            .map_err(|e| format!("Failed to write temporary configuration file: {e}"))?;
        fs::rename(&tmp_path, config_file)
            .map_err(|e| format!("Failed to persist configuration file: {e}"))?;
        Ok(())
    }
}

pub fn normalize_language(lang: &str) -> String {
    let lower = lang.trim().to_ascii_lowercase();
    let short = lower.split(['-', '_']).next().unwrap_or("en");
    match short {
        "de" | "en" | "fr" | "es" | "pl" => short.to_string(),
        _ => "en".to_string(),
    }
}

/// Verifies an access key against salted 10,000-round 128-bit digests so plaintext keys
/// never appear in source code, unit tests, compiled binary `.rdata` string tables, or frontend bundles.
/// Supports optional compile-time CI secret overrides (`GM_EXTENDED_KEY_DIGEST` / `GM_DEVELOPER_KEY_DIGEST`
/// formatted as `"<hex16>:<hex16>"`).
pub fn verify_access_key(raw_key: &str) -> Option<AccessTier> {
    let extended_digest = option_env!("GM_EXTENDED_KEY_DIGEST")
        .and_then(parse_digest_pair_hex)
        .unwrap_or(EXTENDED_TIER_DIGEST);
    let developer_digest = option_env!("GM_DEVELOPER_KEY_DIGEST")
        .and_then(parse_digest_pair_hex)
        .unwrap_or(DEVELOPER_TIER_DIGEST);
    verify_access_key_with_digests(raw_key, extended_digest, developer_digest)
}

/// Verifies `raw_key` against explicit `(u64, u64)` target digests (used by `verify_access_key`
/// and by unit tests with ephemeral dummy digests so real keys never appear in test code).
pub fn verify_access_key_with_digests(
    raw_key: &str,
    extended_digest: (u64, u64),
    developer_digest: (u64, u64),
) -> Option<AccessTier> {
    let candidate = raw_key.trim();
    if candidate.is_empty() || candidate.len() > 128 {
        return None;
    }
    let digest = digest_access_key(candidate);
    if constant_time_eq_128(digest, developer_digest) {
        Some(AccessTier::Developer)
    } else if constant_time_eq_128(digest, extended_digest) {
        Some(AccessTier::Extended)
    } else {
        None
    }
}

fn parse_digest_pair_hex(raw: &str) -> Option<(u64, u64)> {
    let (hi, lo) = raw.trim().split_once(':')?;
    let a = u64::from_str_radix(hi.trim().trim_start_matches("0x"), 16).ok()?;
    let b = u64::from_str_radix(lo.trim().trim_start_matches("0x"), 16).ok()?;
    Some((a, b))
}

fn constant_time_eq_128(a: (u64, u64), b: (u64, u64)) -> bool {
    ((a.0 ^ b.0) | (a.1 ^ b.1)) == 0
}

/// Computes the salted 10,000-round 128-bit digest of a trimmed access key string.
pub fn digest_access_key(trimmed: &str) -> (u64, u64) {
    const SALT_A: &[u8] = b"FS25_GlobalMarket_DSA_AccessTier_A_v1:";
    const SALT_B: &[u8] = b"FS25_GlobalMarket_DSA_AccessTier_B_v1:";

    fn mix64(seed_init: u64, salt: &[u8], payload: &[u8]) -> u64 {
        let mut h = seed_init;
        for &b in salt.iter().chain(payload.iter()) {
            h ^= b as u64;
            h = h.wrapping_mul(0x1000_0000_01B3);
        }
        for round in 0..10_000_u64 {
            h = h.wrapping_add(0x9E37_79B9_7F4A_7C15 ^ round);
            h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            h ^= h >> 31;
        }
        h
    }

    let bytes = trimmed.as_bytes();
    (
        mix64(0xCBF2_9CE4_8422_2325, SALT_A, bytes),
        mix64(0x8422_2325_CBF2_9CE4, SALT_B, bytes),
    )
}

/// Generates a 16-character lowercase hex identifier backed by OS CSPRNG-seeded `RandomState`.
pub fn generate_client_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let seq = CLIENT_ID_NONCE.fetch_add(1, Ordering::Relaxed);

    let mut h1 = RandomState::new().build_hasher();
    h1.write_u64(nanos);
    h1.write_u64(pid);
    h1.write_u64(seq);

    let mut h2 = RandomState::new().build_hasher();
    h2.write_u64(h1.finish());
    h2.write_u64(seq.wrapping_mul(0x9E37_79B9_7F4A_7C15));

    let mixed = h1.finish() ^ h2.finish().rotate_left(13);
    format!("{mixed:016x}")
}

pub fn is_valid_client_id(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed.len() >= 8
        && trimmed.len() <= 64
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Prefixes a file-derived transaction stem (e.g. `"1729512015_1"`) with the persistent
/// `client_id` so two players trading in the same second never collide while retries stay idempotent.
pub fn qualify_transaction_id(client_id: &str, raw_tx_id: &str) -> String {
    let cid = client_id.trim();
    let tid = raw_tx_id.trim();
    if cid.is_empty() {
        return tid.to_string();
    }
    let prefix = format!("{cid}_");
    if tid.starts_with(&prefix) {
        tid.to_string()
    } else {
        format!("{prefix}{tid}")
    }
}
