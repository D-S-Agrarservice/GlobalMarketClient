//! Cross-platform, multi-version Farming Simulator profile discovery & validation
//! (`FarmingSimulator2025`, future `FarmingSimulator2028`, etc. across Windows, macOS, and Linux Proton).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Known Steam AppIDs for Proton prefix scanning (FS25 = `2300320`; unknown future prefixes are scanned dynamically).
const KNOWN_PROTON_APP_IDS: &[&str] = &["2300320"];
const DEFAULT_EDITION_CODE: &str = "FS25";

#[derive(Debug, Clone)]
pub struct BridgePaths {
    /// Detected Farming Simulator edition tag (e.g. `"FS25"`, `"FS28"`).
    pub game_edition: String,
    /// Selected or discovered Farming Simulator profile root (or custom `-profile` folder).
    pub profile_dir: PathBuf,
    /// `.../modSettings/FS<YY>_GlobalMarket`
    pub bridge_root: PathBuf,
    /// `.../modSettings/FS<YY>_GlobalMarket/globalMarketOnline.xml`
    pub online_xml_file: PathBuf,
    /// `.../modSettings/FS<YY>_GlobalMarket/server_whitelist.xml`
    pub server_whitelist_file: PathBuf,
    /// `.../modSettings/FS<YY>_GlobalMarket/read` (EXE -> Lua)
    pub read_dir: PathBuf,
    /// `.../modSettings/FS<YY>_GlobalMarket/write` (Lua -> EXE)
    pub write_dir: PathBuf,
    /// `.../modSettings/FS<YY>_GlobalMarket/logos` (cached `512x256` DXT5 `.dds` community logos)
    pub logos_dir: PathBuf,
    /// `.../modSettings/FS<YY>_GlobalMarket/.staging` (invisible temp staging outside `read/`)
    pub staging_dir: PathBuf,
}

impl BridgePaths {
    pub fn ensure_directories(&self) -> Result<(), String> {
        for dir in [
            &self.bridge_root,
            &self.read_dir,
            &self.write_dir,
            &self.logos_dir,
            &self.staging_dir,
        ] {
            fs::create_dir_all(dir).map_err(|e| {
                format!(
                    "Failed to create bridge directory '{}': {e}",
                    dir.display()
                )
            })?;
        }
        Ok(())
    }
}

/// Automatically searches standard OS locations for an installed Farming Simulator (`>= 2025`) profile directory:
/// - **Windows:** Standard `Documents/My Games` and OneDrive (`Documents` & `Dokumente`)
/// - **macOS:** Steam (`~/Library/Application Support/...`) & Mac App Store Containers
/// - **Linux / SteamOS:** Steam Proton `compatdata/*/pfx/...` across primary & secondary Steam libraries
///
/// Prefers profiles that already contain a `GlobalMarket` mod/settings folder, then newest edition year (`>= 25`).
pub fn discover_fs25_profile_path() -> Option<PathBuf> {
    let mut valid_candidates: Vec<(bool, u16, PathBuf)> = Vec::new();

    for candidate in candidate_profile_directories() {
        if looks_like_fs_profile(&candidate) {
            let year = extract_profile_year(&candidate).unwrap_or(25);
            let edition = format!("FS{year:02}");
            let mod_folder = format!("{edition}_GlobalMarket");
            let has_gm = candidate.join("modSettings").join(&mod_folder).is_dir()
                || candidate
                    .join("mods")
                    .join(format!("{mod_folder}.zip"))
                    .is_file()
                || candidate.join("mods").join(&mod_folder).is_dir();
            valid_candidates.push((has_gm, year, candidate));
        }
    }

    // Sort: profiles with GlobalMarket installed first, then newest FS edition year first.
    valid_candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    valid_candidates.into_iter().next().map(|(_, _, p)| p)
}

/// Validates and resolves a user-chosen or auto-discovered folder into [`BridgePaths`].
///
/// Supports `FarmingSimulator2025` as well as future editions (e.g. `FarmingSimulator2028`), accepting:
/// 1. The FS profile root (`.../FarmingSimulator2025`, `.../FarmingSimulator2028`, or custom `-profile` folder)
/// 2. The `modSettings` subfolder directly
/// 3. The `FS<YY>_GlobalMarket` bridge folder directly (e.g. `FS25_GlobalMarket` or `FS28_GlobalMarket`)
pub fn resolve_bridge_paths(raw_selected: &Path) -> Result<BridgePaths, String> {
    if !raw_selected.exists() || !raw_selected.is_dir() {
        return Err(format!(
            "Directory '{}' does not exist.",
            raw_selected.display()
        ));
    }

    let folder_name = raw_selected
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    let (game_edition, profile_dir, bridge_root) =
        if let Some(edition) = parse_mod_folder_edition(folder_name) {
            let mod_settings = raw_selected.parent().unwrap_or(raw_selected);
            let profile = mod_settings.parent().unwrap_or(mod_settings).to_path_buf();
            (edition, profile, raw_selected.to_path_buf())
        } else if folder_name.eq_ignore_ascii_case("modSettings") {
            let profile = raw_selected.parent().unwrap_or(raw_selected).to_path_buf();
            let edition = detect_edition_for_profile(&profile);
            let mod_folder = format!("{edition}_GlobalMarket");
            (edition, profile, raw_selected.join(mod_folder))
        } else if looks_like_fs_profile(raw_selected) {
            let profile = raw_selected.to_path_buf();
            let edition = detect_edition_for_profile(&profile);
            let mod_folder = format!("{edition}_GlobalMarket");
            let bridge = profile.join("modSettings").join(mod_folder);
            (edition, profile, bridge)
        } else {
            return Err(
                "Invalid directory: Please select your Farming Simulator profile folder (e.g. 'FarmingSimulator2025' containing gameSettings.xml / modSettings) or 'modSettings/FS25_GlobalMarket'."
                    .to_string(),
            );
        };

    Ok(BridgePaths {
        game_edition,
        online_xml_file: bridge_root.join("globalMarketOnline.xml"),
        server_whitelist_file: bridge_root.join("server_whitelist.xml"),
        read_dir: bridge_root.join("read"),
        write_dir: bridge_root.join("write"),
        logos_dir: bridge_root.join("logos"),
        staging_dir: bridge_root.join(".staging"),
        profile_dir,
        bridge_root,
    })
}

/// Parses `"FS25_GlobalMarket"` -> `Some("FS25")`, `"FS28_GlobalMarket"` -> `Some("FS28")`.
pub fn parse_mod_folder_edition(folder_name: &str) -> Option<String> {
    let trimmed = folder_name.trim();
    let (prefix, suffix) = trimmed.split_once('_')?;
    if !suffix.eq_ignore_ascii_case("GlobalMarket") {
        return None;
    }
    let upper = prefix.to_ascii_uppercase();
    let digits = upper.strip_prefix("FS")?;
    if digits.len() == 2 && digits.chars().all(|c| c.is_ascii_digit()) {
        let yr: u16 = digits.parse().ok()?;
        if yr >= 25 {
            return Some(format!("FS{yr:02}"));
        }
    }
    None
}

/// Extracts 2-digit edition year (`25`, `28`, ...) from `"FarmingSimulator2025"` / `"FarmingSimulator2028"`.
fn extract_profile_year(dir: &Path) -> Option<u16> {
    let name = dir.file_name()?.to_str()?.trim();
    let lower = name.to_ascii_lowercase();
    let year_str = lower.strip_prefix("farmingsimulator20")?;
    if year_str.len() == 2 && year_str.chars().all(|c| c.is_ascii_digit()) {
        let yr: u16 = year_str.parse().ok()?;
        if yr >= 25 {
            return Some(yr);
        }
    }
    None
}

/// Determines `"FS25"` / `"FS28"` from the profile folder name, or by inspecting `modSettings/` & `mods/`
/// when a custom `-profile` directory name is used.
fn detect_edition_for_profile(profile_dir: &Path) -> String {
    if let Some(yr) = extract_profile_year(profile_dir) {
        return format!("FS{yr:02}");
    }

    // Custom `-profile` directory: inspect `modSettings/FS<YY>_GlobalMarket` first, then `mods/FS<YY>_GlobalMarket(.zip)`.
    let mut detected_years: Vec<u16> = Vec::new();
    if let Ok(entries) = fs::read_dir(profile_dir.join("modSettings")) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(ed) = parse_mod_folder_edition(name) {
                        if let Ok(yr) = ed[2..].parse::<u16>() {
                            detected_years.push(yr);
                        }
                    }
                }
            }
        }
    }

    if detected_years.is_empty() {
        if let Ok(entries) = fs::read_dir(profile_dir.join("mods")) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    let stem = name.strip_suffix(".zip").unwrap_or(name);
                    if let Some(ed) = parse_mod_folder_edition(stem) {
                        if let Ok(yr) = ed[2..].parse::<u16>() {
                            detected_years.push(yr);
                        }
                    }
                }
            }
        }
    }

    detected_years.sort_unstable_by(|a, b| b.cmp(a));
    if let Some(yr) = detected_years.first() {
        format!("FS{yr:02}")
    } else {
        DEFAULT_EDITION_CODE.to_string()
    }
}

fn looks_like_fs_profile(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let name_matches = extract_profile_year(dir).is_some();

    let has_fs_markers = dir.join("game.xml").is_file()
        || dir.join("gameSettings.xml").is_file()
        || dir.join("modSettings").is_dir()
        || dir.join("mods").is_dir();

    name_matches || has_fs_markers
}

fn collect_fs_subdirs(parent: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let path = entry.path();
            if extract_profile_year(&path).is_some() && path.is_dir() {
                out.push(path);
            }
        }
    }
    // Always include the explicit FS25 path as fallback if directory listing was restricted.
    let fs25 = parent.join("FarmingSimulator2025");
    if !out.contains(&fs25) {
        out.push(fs25);
    }
}

fn candidate_profile_directories() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // 1. Windows (OneDrive Consumer / Business + Standard UserProfile Documents/Dokumente)
    for env_key in ["OneDriveConsumer", "OneDrive", "USERPROFILE"] {
        if let Ok(base) = env::var(env_key) {
            let base_path = PathBuf::from(base);
            for docs_name in ["Documents", "Dokumente"] {
                let my_games = base_path.join(docs_name).join("My Games");
                collect_fs_subdirs(&my_games, &mut candidates);
            }
        }
    }

    // 2. macOS & Linux ($HOME based locations)
    if let Ok(home) = env::var("HOME") {
        let home_path = PathBuf::from(home);

        // macOS (Steam / Giants Standalone)
        let app_support = home_path.join("Library").join("Application Support");
        collect_fs_subdirs(&app_support, &mut candidates);

        // macOS (Mac App Store sandbox containers: com.giants-software.farmingsim25, farmingsim28, ...)
        let containers_dir = home_path.join("Library").join("Containers");
        if let Ok(entries) = fs::read_dir(&containers_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with("com.giants-software.farmingsim") {
                        let inner_support = entry
                            .path()
                            .join("Data")
                            .join("Library")
                            .join("Application Support");
                        collect_fs_subdirs(&inner_support, &mut candidates);
                    }
                }
            }
        }

        // Linux / Steam Deck (Proton prefixes across common Steam roots + libraryfolders.vdf)
        let mut steam_roots = vec![
            home_path.join(".steam").join("steam"),
            home_path.join(".local").join("share").join("Steam"),
            home_path
                .join(".var")
                .join("app")
                .join("com.valvesoftware.Steam")
                .join(".local")
                .join("share")
                .join("Steam"),
        ];

        let mut extra_libs = Vec::new();
        for root in &steam_roots {
            let vdf = root.join("steamapps").join("libraryfolders.vdf");
            if let Ok(content) = fs::read_to_string(vdf) {
                extra_libs.extend(parse_steam_library_paths(&content));
            }
        }
        steam_roots.extend(extra_libs);

        for root in steam_roots {
            let compat_root = root.join("steamapps").join("compatdata");
            // Scan known Proton AppIDs first, then any other prefix containing a FarmingSimulator20XX folder.
            for app_id in KNOWN_PROTON_APP_IDS {
                let my_games = compat_root
                    .join(app_id)
                    .join("pfx")
                    .join("drive_c")
                    .join("users")
                    .join("steamuser")
                    .join("Documents")
                    .join("My Games");
                collect_fs_subdirs(&my_games, &mut candidates);
            }
            if let Ok(entries) = fs::read_dir(&compat_root) {
                for entry in entries.flatten() {
                    let my_games = entry
                        .path()
                        .join("pfx")
                        .join("drive_c")
                        .join("users")
                        .join("steamuser")
                        .join("Documents")
                        .join("My Games");
                    if my_games.is_dir() {
                        collect_fs_subdirs(&my_games, &mut candidates);
                    }
                }
            }
        }
    }

    candidates
}

fn parse_steam_library_paths(vdf_content: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in vdf_content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("\"path\"") {
            let val = rest.trim().trim_matches('"').replace("\\\\", "\\");
            if !val.is_empty() {
                paths.push(PathBuf::from(val));
            }
        }
    }
    paths
}
