//! OS-protected local storage for Community & Logo market tokens (`tokens.vault`).
//!
//! Security properties (designed for an open-source public repository):
//! - **Zero embedded secrets in source code**: On Windows, encryption and integrity are delegated
//!   directly to the OS Data Protection API (`CryptProtectData` / `CryptUnprotectData` in `crypt32.dll`
//!   with `CRYPTPROTECT_UI_FORBIDDEN = 0x01`), binding `tokens.vault` cryptographically to the
//!   current Windows user session with **zero UAC prompts and zero OS dialogs**.
//! - **Strict Write-Only IPC**: Raw token strings never leave Rust over Tauri IPC and are never
//!   written to `settings.json`; the WebView receives only `SavedTokenProfile` metadata (`id`,
//!   optional label, server-detected market name, branding tier, and masked suffix `••••xxxx`).
//! - **Cross-platform fallback**: On macOS and Linux, `tokens.vault` is stored outside `settings.json`
//!   with strict owner-only `0600` (`rw-------`) POSIX file permissions and SHA-256 integrity verification.
//! - The same vault keeps the savegame secrets (Spec Section 3.5) and the optional developer token
//!   for the development server (`X-Dev-Token`), with the same write-only IPC rule.

use crate::api::types::ServerSecret;
use crate::bridge::identity::ServerIdentity;
use crate::bridge::xml::{sanitize_logo_rel_path, sanitize_market_name};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

static VAULT_TMP_SEQ: AtomicU64 = AtomicU64::new(1);

/// Maximum number of token profiles stored locally per installation.
pub const MAX_SAVED_TOKENS: usize = 32;

/// Maximum length (in Unicode scalar values) of an optional user-defined profile label.
pub const MAX_CUSTOM_LABEL_CHARS: usize = 36;

/// Maximum accepted raw token byte length.
pub const MAX_RAW_TOKEN_BYTES: usize = 512;

const VAULT_SCHEMA_VERSION: u8 = 1;

/// Maximum length of a developer token (`dev_` plus 32 URL-safe characters on the backend).
pub const MAX_DEV_TOKEN_CHARS: usize = 128;

/// Public, non-sensitive metadata for a saved market/logo token exposed to the TypeScript UI.
/// Never contains the raw token secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedTokenProfile {
    /// Deterministic 12-character hex identifier derived via SHA-256 of the trimmed token.
    pub id: String,
    /// Optional user-chosen nickname (e.g. `"Stammtisch"` or `"Unser Community-Logo"`).
    pub custom_label: Option<String>,
    /// Server-reported market name learned after a successful private-market handshake (Tier 2/3).
    pub detected_market_name: Option<String>,
    /// Last verified branding tier (`0..=3`), or `None` before the first successful handshake.
    pub branding_tier: Option<u8>,
    /// Whether this token belongs to an isolated Private Market (`true` for Tier 2/3, `false` for Tier 1).
    pub is_private_market: bool,
    /// Irreversible display hint showing at most the last 4 characters (`"••••a9f2"` or `"••••••"`).
    pub masked_hint: String,
    /// Remembered relative `.dds` cache path (`logos/logo_<hash>.dds`) for instant switching in FS25.
    pub cached_logo_file: Option<String>,
    /// Remembered validated logo URL for instant client header preview when switching profiles.
    pub cached_logo_url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct VaultEntry {
    profile: SavedTokenProfile,
    raw_token: String,
}

impl Drop for VaultEntry {
    fn drop(&mut self) {
        zeroize_string(&mut self.raw_token);
    }
}

/// Savegame secret issued by the backend (Spec Section 3.5), kept so a client exit or a game closed
/// without saving cannot lock the savegame out.
#[derive(Clone, Serialize, Deserialize)]
struct VaultServerIdentity {
    scope: String,
    server_id: String,
    secret: String,
    #[serde(default)]
    known_to_mod: bool,
}

impl Drop for VaultServerIdentity {
    fn drop(&mut self) {
        zeroize_string(&mut self.secret);
    }
}

/// Developer token for the development server (`X-Dev-Token`), zeroized on drop.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
struct VaultDevToken(String);

impl Drop for VaultDevToken {
    fn drop(&mut self) {
        zeroize_string(&mut self.0);
    }
}

#[derive(Serialize, Deserialize)]
struct VaultPayload {
    version: u8,
    entries: Vec<VaultEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    server_identities: Vec<VaultServerIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dev_token: Option<VaultDevToken>,
}

impl Default for VaultPayload {
    fn default() -> Self {
        Self {
            version: VAULT_SCHEMA_VERSION,
            entries: Vec::new(),
            server_identities: Vec::new(),
            dev_token: None,
        }
    }
}

/// Manages the encrypted `tokens.vault` file alongside `settings.json`.
pub struct TokenVault {
    vault_file: PathBuf,
    entries: Vec<VaultEntry>,
    server_identities: Vec<VaultServerIdentity>,
    dev_token: Option<VaultDevToken>,
}

impl TokenVault {
    /// Resolves the canonical sibling `tokens.vault` path next to `settings.json`.
    pub fn vault_path_for_config(config_file: &Path) -> PathBuf {
        config_file.with_file_name("tokens.vault")
    }

    /// Loads and decrypts `tokens.vault` if present, falling back cleanly to an empty vault
    /// if the file does not exist yet or cannot be decrypted (e.g. copied from another machine).
    pub fn load_or_empty(vault_file: PathBuf) -> Self {
        let payload = fs::read(&vault_file)
            .ok()
            .and_then(|cipher| unseal_vault_bytes(&cipher).ok())
            .and_then(|mut plain| {
                let parsed = serde_json::from_slice::<VaultPayload>(&plain).ok();
                zeroize_bytes(&mut plain);
                parsed
            })
            .unwrap_or_default();

        Self {
            vault_file,
            entries: payload.entries,
            server_identities: payload.server_identities,
            dev_token: payload.dev_token,
        }
    }

    /// Encrypts and atomically persists the current vault to `tokens.vault` (or removes the file when empty).
    pub fn save(&self) -> Result<(), String> {
        if self.entries.is_empty() && self.server_identities.is_empty() && self.dev_token.is_none()
        {
            if self.vault_file.exists() {
                let _ = fs::remove_file(&self.vault_file);
            }
            return Ok(());
        }

        if let Some(parent) = self.vault_file.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create token vault directory: {e}"))?;
        }

        let payload = VaultPayload {
            version: VAULT_SCHEMA_VERSION,
            entries: self.entries.clone(),
            server_identities: self.server_identities.clone(),
            dev_token: self.dev_token.clone(),
        };
        let mut plain = serde_json::to_vec(&payload)
            .map_err(|e| format!("Failed to serialize token vault: {e}"))?;
        let sealed_res = seal_vault_bytes(&plain);
        zeroize_bytes(&mut plain);
        let sealed = sealed_res?;

        atomic_write_restricted(&self.vault_file, &sealed)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns safe, non-sensitive metadata for all saved tokens (in insertion order).
    pub fn list_profiles(&self) -> Vec<SavedTokenProfile> {
        self.entries.iter().map(|e| e.profile.clone()).collect()
    }

    /// Returns safe metadata for a single token profile by `id`.
    pub fn get_profile(&self, id: &str) -> Option<SavedTokenProfile> {
        let clean_id = id.trim();
        self.entries
            .iter()
            .find(|e| e.profile.id == clean_id)
            .map(|e| e.profile.clone())
    }

    /// Resolves the raw secret token for internal Rust HTTP authentication (`X-Market-Token`).
    /// Never expose this return value to IPC or disk logs!
    pub(crate) fn resolve_raw_token(&self, id: &str) -> Option<String> {
        let clean_id = id.trim();
        self.entries
            .iter()
            .find(|e| e.profile.id == clean_id)
            .map(|e| e.raw_token.clone())
    }

    /// Collects all stored raw tokens exclusively so diagnostic report generation can scrub
    /// accidental echoes from error strings or logs.
    pub(crate) fn all_raw_tokens(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|e| e.raw_token.clone())
            .chain(self.dev_token.iter().map(|t| t.0.clone()))
            .collect()
    }

    /// The developer token for `X-Dev-Token`. Never expose it to IPC or disk logs!
    pub(crate) fn dev_token(&self) -> Option<String> {
        self.dev_token.as_ref().map(|t| t.0.clone())
    }

    pub fn has_dev_token(&self) -> bool {
        self.dev_token.is_some()
    }

    /// Stores (`Some`) or removes (`None`) the developer token (call `save()` afterwards).
    pub fn set_dev_token(&mut self, raw: Option<&str>) -> Result<(), String> {
        self.dev_token = raw.map(validate_dev_token).transpose()?.map(VaultDevToken);
        Ok(())
    }

    /// Persisted savegame identities. Never expose the secrets to IPC or disk logs!
    pub(crate) fn server_identities(&self) -> Vec<ServerIdentity> {
        self.server_identities
            .iter()
            .filter_map(|e| {
                Some(ServerIdentity {
                    scope: e.scope.clone(),
                    server_id: e.server_id.clone(),
                    secret: ServerSecret::parse(&e.secret)?,
                    issued: true,
                    known_to_mod: e.known_to_mod,
                    rejected: false,
                })
            })
            .collect()
    }

    /// Replaces the persisted savegame identities (call `save()` afterwards).
    pub(crate) fn set_server_identities(&mut self, identities: &[ServerIdentity]) {
        self.server_identities = identities
            .iter()
            .map(|i| VaultServerIdentity {
                scope: i.scope.clone(),
                server_id: i.server_id.clone(),
                secret: i.secret.expose().to_string(),
                known_to_mod: i.known_to_mod,
            })
            .collect();
    }

    /// Validates and inserts (or deduplicates/updates) a market token in the vault.
    pub fn upsert_token(
        &mut self,
        raw_token: &str,
        custom_label: Option<&str>,
    ) -> Result<SavedTokenProfile, String> {
        let cleaned_token = validate_raw_token(raw_token)?;
        let cleaned_label = custom_label.and_then(sanitize_custom_label);
        let id = compute_token_profile_id(&cleaned_token);
        let masked_hint = build_masked_hint(&cleaned_token);

        if let Some(existing) = self.entries.iter_mut().find(|e| e.profile.id == id) {
            existing.raw_token = cleaned_token;
            existing.profile.masked_hint = masked_hint;
            if cleaned_label.is_some() {
                existing.profile.custom_label = cleaned_label;
            }
            return Ok(existing.profile.clone());
        }

        if self.entries.len() >= MAX_SAVED_TOKENS {
            return Err(format!(
                "Maximum of {MAX_SAVED_TOKENS} saved market tokens reached."
            ));
        }

        let profile = SavedTokenProfile {
            id,
            custom_label: cleaned_label,
            detected_market_name: None,
            branding_tier: None,
            is_private_market: false,
            masked_hint,
            cached_logo_file: None,
            cached_logo_url: None,
        };

        self.entries.push(VaultEntry {
            profile: profile.clone(),
            raw_token: cleaned_token,
        });
        Ok(profile)
    }

    /// Updates the optional user-chosen nickname for an existing profile without touching the secret.
    pub fn update_custom_label(&mut self, id: &str, custom_label: Option<&str>) -> bool {
        let clean_id = id.trim();
        let next_label = custom_label.and_then(sanitize_custom_label);
        if let Some(entry) = self.entries.iter_mut().find(|e| e.profile.id == clean_id) {
            if entry.profile.custom_label != next_label {
                entry.profile.custom_label = next_label;
                return true;
            }
        }
        false
    }

    /// Removes a saved token profile by `id`. Returns `true` if an entry was removed.
    pub fn remove_token(&mut self, id: &str) -> bool {
        let clean_id = id.trim();
        let before = self.entries.len();
        self.entries.retain(|e| e.profile.id != clean_id);
        self.entries.len() != before
    }

    /// Updates server-learned branding metadata after a live `/api/v1/meta/status` handshake.
    /// Returns `true` if any field changed so callers only persist `tokens.vault` on actual changes.
    pub fn record_handshake_metadata(
        &mut self,
        id: &str,
        branding_tier: u8,
        is_private_market: bool,
        detected_market_name: Option<&str>,
        logo_url: Option<&str>,
        logo_file: Option<&str>,
    ) -> bool {
        let clean_id = id.trim();
        let Some(entry) = self.entries.iter_mut().find(|e| e.profile.id == clean_id) else {
            return false;
        };

        let next_tier = Some(branding_tier.min(3));
        let next_priv = is_private_market && matches!(branding_tier, 2 | 3);
        let next_market = if next_priv {
            detected_market_name.and_then(sanitize_market_name)
        } else {
            None
        };
        let next_logo_file = if matches!(branding_tier, 1 | 3) {
            logo_file.and_then(sanitize_logo_rel_path)
        } else {
            None
        };
        let next_logo_url = if matches!(branding_tier, 1 | 3) {
            logo_url
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(ToOwned::to_owned)
        } else {
            None
        };

        let mut changed = false;
        if entry.profile.branding_tier != next_tier {
            entry.profile.branding_tier = next_tier;
            changed = true;
        }
        if entry.profile.is_private_market != next_priv {
            entry.profile.is_private_market = next_priv;
            changed = true;
        }
        if entry.profile.detected_market_name != next_market {
            entry.profile.detected_market_name = next_market;
            changed = true;
        }
        if entry.profile.cached_logo_file != next_logo_file {
            entry.profile.cached_logo_file = next_logo_file;
            changed = true;
        }
        if entry.profile.cached_logo_url != next_logo_url {
            entry.profile.cached_logo_url = next_logo_url;
            changed = true;
        }
        changed
    }
}

/// Computes a deterministic 12-character lowercase hex ID from a trimmed market token.
pub fn compute_token_profile_id(trimmed_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"FS25_GM_TokenProfile_v1:");
    hasher.update(trimmed_token.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(12);
    for &b in &digest[..6] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Formats an irreversible masked suffix (`"••••a9f2"` for tokens `>= 8` chars, `"••••••"` otherwise).
pub fn build_masked_hint(trimmed_token: &str) -> String {
    let chars: Vec<char> = trimmed_token.chars().collect();
    if chars.len() >= 8 {
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("••••{suffix}")
    } else {
        "••••••".to_string()
    }
}

/// Sanitizes an optional user-chosen token nickname.
pub fn sanitize_custom_label(raw: &str) -> Option<String> {
    let clean: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if clean.is_empty() {
        return None;
    }
    let truncated: String = clean.chars().take(MAX_CUSTOM_LABEL_CHARS).collect();
    let trimmed = truncated.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn validate_raw_token(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Token must not be empty.".to_string());
    }
    if trimmed.len() > MAX_RAW_TOKEN_BYTES {
        return Err("Token exceeds maximum allowed length.".to_string());
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("Token contains invalid control characters.".to_string());
    }
    Ok(trimmed.to_string())
}

/// A developer token must be a valid HTTP header value: visible ASCII only, no spaces.
fn validate_dev_token(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Developer token must not be empty.".to_string());
    }
    if trimmed.len() > MAX_DEV_TOKEN_CHARS {
        return Err("Developer token exceeds maximum allowed length.".to_string());
    }
    if !trimmed.chars().all(|c| c.is_ascii_graphic()) {
        return Err("Developer token contains invalid characters.".to_string());
    }
    Ok(trimmed.to_string())
}

fn zeroize_bytes(buf: &mut [u8]) {
    for byte in buf.iter_mut() {
        unsafe {
            std::ptr::write_volatile(byte, 0);
        }
    }
    std::sync::atomic::compiler_fence(Ordering::SeqCst);
}

fn zeroize_string(s: &mut String) {
    // SAFETY: Overwriting UTF-8 bytes with ASCII NUL (`0x00`) preserves valid UTF-8 before clearing.
    unsafe {
        zeroize_bytes(s.as_bytes_mut());
    }
    s.clear();
}

fn atomic_write_restricted(target_path: &Path, data: &[u8]) -> Result<(), String> {
    let seq = VAULT_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = target_path.with_extension(format!("vault.{seq}.tmp"));

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)
            .map_err(|e| format!("Failed to create restricted vault temp file: {e}"))?;
        file.write_all(data)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Failed to write vault temp file: {e}"))?;
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut file = fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create vault temp file: {e}"))?;
        file.write_all(data)
            .and_then(|_| file.sync_all())
            .map_err(|e| format!("Failed to write vault temp file: {e}"))?;
    }

    let mut last_err = None;
    for attempt in 0..5 {
        match fs::rename(&tmp_path, target_path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                thread::sleep(Duration::from_millis(15 * (attempt + 1)));
            }
        }
    }
    let _ = fs::remove_file(&tmp_path);
    Err(format!(
        "Failed to persist token vault {}: {}",
        target_path.display(),
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    ))
}

#[cfg(target_os = "windows")]
mod os_seal {
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::atomic::Ordering;

    const VAULT_MAGIC_WIN: &[u8; 8] = b"GMV1WIN\0";
    /// Public domain-separation label passed as `pOptionalEntropy` to Windows DPAPI so unrelated
    /// local tools cannot accidentally unseal `tokens.vault`. Security relies 100% on the OS-managed
    /// Windows user master key (`CryptProtectData`), never on this public tag.
    const DPAPI_DOMAIN_TAG: &[u8] = b"FS25_GlobalMarketClient_DPAPI_v1";
    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x01;

    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            p_data_in: *const DataBlob,
            sz_data_descr: *const u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *const c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;

        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_data_descr: *mut *mut u16,
            p_optional_entropy: *const DataBlob,
            pv_reserved: *mut c_void,
            p_prompt_struct: *const c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    pub fn seal(plaintext: &[u8]) -> Result<Vec<u8>, String> {
        if plaintext.is_empty() {
            return Err("Cannot seal empty vault payload.".to_string());
        }
        let in_blob = DataBlob {
            cb_data: plaintext.len() as u32,
            pb_data: plaintext.as_ptr() as *mut u8,
        };
        let entropy_blob = DataBlob {
            cb_data: DPAPI_DOMAIN_TAG.len() as u32,
            pb_data: DPAPI_DOMAIN_TAG.as_ptr() as *mut u8,
        };
        let mut out_blob = DataBlob {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };

        // SAFETY: `in_blob` and `entropy_blob` point to valid immutable slices for the duration of
        // the synchronous Win32 call; `out_blob` is freed via `LocalFree` immediately after copy.
        let ok = unsafe {
            CryptProtectData(
                &in_blob,
                ptr::null(),
                &entropy_blob,
                ptr::null_mut(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };
        if ok == 0 || out_blob.pb_data.is_null() || out_blob.cb_data == 0 {
            return Err("Windows DPAPI CryptProtectData failed.".to_string());
        }

        let len = out_blob.cb_data as usize;
        let mut result = Vec::with_capacity(VAULT_MAGIC_WIN.len() + len);
        result.extend_from_slice(VAULT_MAGIC_WIN);
        unsafe {
            let cipher_slice = std::slice::from_raw_parts(out_blob.pb_data, len);
            result.extend_from_slice(cipher_slice);
            let _ = LocalFree(out_blob.pb_data as *mut c_void);
        }
        Ok(result)
    }

    pub fn unseal(sealed: &[u8]) -> Result<Vec<u8>, String> {
        if sealed.len() <= VAULT_MAGIC_WIN.len() || &sealed[..8] != VAULT_MAGIC_WIN {
            return Err("Invalid or unrecognized Windows DPAPI vault header.".to_string());
        }
        let cipher = &sealed[VAULT_MAGIC_WIN.len()..];
        let in_blob = DataBlob {
            cb_data: cipher.len() as u32,
            pb_data: cipher.as_ptr() as *mut u8,
        };
        let entropy_blob = DataBlob {
            cb_data: DPAPI_DOMAIN_TAG.len() as u32,
            pb_data: DPAPI_DOMAIN_TAG.as_ptr() as *mut u8,
        };
        let mut out_blob = DataBlob {
            cb_data: 0,
            pb_data: ptr::null_mut(),
        };

        // SAFETY: `in_blob` and `entropy_blob` point to valid slices; on success `out_blob` is
        // copied, scrubbed via `write_volatile`, and released with `LocalFree`.
        let ok = unsafe {
            CryptUnprotectData(
                &in_blob,
                ptr::null_mut(),
                &entropy_blob,
                ptr::null_mut(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out_blob,
            )
        };
        if ok == 0 || out_blob.pb_data.is_null() {
            return Err("Windows DPAPI CryptUnprotectData rejected vault data.".to_string());
        }

        let len = out_blob.cb_data as usize;
        let mut plain = vec![0u8; len];
        unsafe {
            ptr::copy_nonoverlapping(out_blob.pb_data, plain.as_mut_ptr(), len);
            for i in 0..len {
                ptr::write_volatile(out_blob.pb_data.add(i), 0);
            }
            std::sync::atomic::compiler_fence(Ordering::SeqCst);
            let _ = LocalFree(out_blob.pb_data as *mut c_void);
        }
        Ok(plain)
    }
}

#[cfg(not(target_os = "windows"))]
mod os_seal {
    use sha2::{Digest, Sha256};

    const VAULT_MAGIC_UNIX: &[u8; 8] = b"GMV1UNX\0";
    const UNIX_INTEGRITY_TAG: &[u8] = b"FS25_GlobalMarketClient_UnixVault_v1:";

    pub fn seal(plaintext: &[u8]) -> Result<Vec<u8>, String> {
        if plaintext.is_empty() {
            return Err("Cannot seal empty vault payload.".to_string());
        }
        let mut mac = Sha256::new();
        mac.update(UNIX_INTEGRITY_TAG);
        mac.update(plaintext);
        let tag = mac.finalize();

        let mut out = Vec::with_capacity(VAULT_MAGIC_UNIX.len() + 32 + plaintext.len());
        out.extend_from_slice(VAULT_MAGIC_UNIX);
        out.extend_from_slice(&tag);
        out.extend_from_slice(plaintext);
        Ok(out)
    }

    pub fn unseal(sealed: &[u8]) -> Result<Vec<u8>, String> {
        if sealed.len() <= VAULT_MAGIC_UNIX.len() + 32 || &sealed[..8] != VAULT_MAGIC_UNIX {
            return Err("Invalid Unix vault header.".to_string());
        }
        let expected_tag = &sealed[8..40];
        let body = &sealed[40..];

        let mut mac = Sha256::new();
        mac.update(UNIX_INTEGRITY_TAG);
        mac.update(body);
        let actual_tag = mac.finalize();
        if actual_tag.as_slice() != expected_tag {
            return Err("Unix vault integrity check failed.".to_string());
        }
        Ok(body.to_vec())
    }
}

/// Seals raw vault JSON bytes using OS-native protection (Windows DPAPI / Unix restricted envelope).
pub fn seal_vault_bytes(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    os_seal::seal(plaintext)
}

/// Unseals raw vault JSON bytes and verifies OS cryptographic integrity.
pub fn unseal_vault_bytes(sealed: &[u8]) -> Result<Vec<u8>, String> {
    os_seal::unseal(sealed)
}
