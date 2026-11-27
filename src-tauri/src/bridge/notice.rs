//! Server notices and maintenance status (Spec Section 3.1.3): persistence of the last notice and
//! classification of backend outages into `maintenance` / `unreachable`.

use crate::api::types::ServerNotice;
use crate::api::ApiError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Why the backend is offline, as reported in `globalMarketOnline#offlineReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineReason {
    /// Announced maintenance or restart (`503 MAINTENANCE`, or an outage inside the announced window).
    Maintenance,
    /// Network error outside any announced window.
    Unreachable,
    /// `403 CONTENT_NOT_SUPPORTED`: the backend refuses the loaded map.
    UnsupportedContent,
}

impl OfflineReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Maintenance => "maintenance",
            Self::Unreachable => "unreachable",
            Self::UnsupportedContent => "unsupportedContent",
        }
    }
}

/// Last known server notice, kept in `server_notice.json` next to `settings.json`, so a client restart
/// during maintenance still knows the announced window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoredServerNotice {
    pub notice: Option<ServerNotice>,
    /// Id of the notice that was already announced (log entry and OS notification).
    pub notified_id: Option<String>,
}

impl StoredServerNotice {
    pub fn path_for_config(config_file: &Path) -> PathBuf {
        config_file.with_file_name("server_notice.json")
    }

    /// Loads the stored notice and drops it when it has already expired.
    pub fn load(path: &Path, now_unix: u64) -> Self {
        let mut stored: Self = fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        if stored.notice.as_ref().is_some_and(|n| n.is_expired(now_unix)) {
            stored.notice = None;
        }
        stored
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| format!("Failed to write {}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| format!("Failed to replace {}: {e}", path.display()))
    }
}

/// Classifies a failed status check. Returns `None` for errors that are not outages (e.g. a rejected
/// key), so the heartbeat keeps the plain offline behavior.
pub fn classify_outage(
    err: &ApiError,
    notice: Option<&ServerNotice>,
    now_unix: u64,
) -> Option<OfflineReason> {
    match err {
        ApiError::Maintenance { .. } => Some(OfflineReason::Maintenance),
        ApiError::ContentNotSupported { .. } => Some(OfflineReason::UnsupportedContent),
        ApiError::Network(_) => {
            if notice.is_some_and(|n| n.covers_outage_at(now_unix)) {
                Some(OfflineReason::Maintenance)
            } else {
                Some(OfflineReason::Unreachable)
            }
        }
        _ => None,
    }
}

/// Merges a `503 MAINTENANCE` answer into the known notice: keeps a matching announced notice (with the
/// newer `until`), otherwise creates a maintenance notice that starts now.
pub fn notice_for_maintenance_error(
    current: Option<&ServerNotice>,
    notice_id: Option<&str>,
    until: Option<u64>,
    now_unix: u64,
) -> ServerNotice {
    let until = until.filter(|u| *u > now_unix);
    if let Some(known) = current.filter(|n| {
        n.is_downtime() && notice_id.map_or(true, |id| id == n.id) && !n.is_expired(now_unix)
    }) {
        let mut merged = known.clone();
        if until.is_some() {
            merged.ends_at = until;
        }
        return merged;
    }
    ServerNotice {
        id: notice_id
            .map(str::to_string)
            .unwrap_or_else(|| format!("maintenance-{}", until.unwrap_or(now_unix))),
        notice_type: "maintenance".to_string(),
        starts_at: Some(now_unix),
        ends_at: until,
        message: Default::default(),
    }
}

/// Language preference for the notice `message`: game language (from the last whitelist file),
/// then client language. `cz` and `cs` are treated as the same language.
pub fn message_languages(game_lang: Option<&str>, ui_lang: &str) -> Vec<String> {
    let mut langs = Vec::new();
    for lang in game_lang.into_iter().chain(std::iter::once(ui_lang)) {
        let lang = lang.trim().to_ascii_lowercase();
        if lang.is_empty() {
            continue;
        }
        let variants = match lang.as_str() {
            "cz" | "cs" => vec!["cs".to_string(), "cz".to_string()],
            _ => vec![lang],
        };
        for l in variants {
            if !langs.contains(&l) {
                langs.push(l);
            }
        }
    }
    langs
}
