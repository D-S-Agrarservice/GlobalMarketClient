//! Savegame identity on the shared online pool (`X-Server-Secret`, Spec Section 3.5).
//!
//! The backend issues a secret once per `server_id`. The mod stores it in the savegame and sends it with
//! every action. Until the mod has proven that by sending it, the client must not lose it: the first
//! authenticated poll already confirms the secret on the server, so it is never issued again.

use crate::api::types::ServerSecret;

/// Maximum number of savegames whose identity is remembered (oldest entries are dropped first).
pub const MAX_SERVER_IDENTITIES: usize = 64;

/// Identity of one savegame on one backend (`scope`), so a secret never reaches another backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerIdentity {
    pub scope: String,
    pub server_id: String,
    pub secret: ServerSecret,
    /// Issued to this client by the backend (`true`) or only learned from the mod (`false`).
    /// Only issued secrets are persisted and passed on to the mod.
    pub issued: bool,
    /// The mod's last action carried this secret, so the savegame has it.
    pub known_to_mod: bool,
    /// The backend rejected this secret (`403 SERVER_SECRET_INVALID`).
    pub rejected: bool,
}

/// Identity state of a savegame for the snapshot and the diagnostic report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerIdentityState {
    Unknown,
    Known,
    Rejected,
}

impl ServerIdentityState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Known => "known",
            Self::Rejected => "rejected",
        }
    }
}

/// All savegame identities known to this client, newest last.
#[derive(Debug, Default)]
pub struct ServerIdentityStore {
    entries: Vec<ServerIdentity>,
    /// Savegames that were rejected without any known secret (`(scope, server_id)`).
    rejected_without_secret: Vec<(String, String)>,
}

fn normalize_id(server_id: &str) -> String {
    server_id.trim().to_ascii_lowercase()
}

impl ServerIdentityStore {
    /// Restores the identities persisted in the vault.
    pub fn from_entries(entries: Vec<ServerIdentity>) -> Self {
        Self {
            entries,
            rejected_without_secret: Vec::new(),
        }
    }

    /// Issued identities, the only ones that are persisted.
    pub fn issued_entries(&self) -> Vec<ServerIdentity> {
        self.entries.iter().filter(|e| e.issued).cloned().collect()
    }

    /// All known secrets, only for scrubbing diagnostic reports.
    pub fn all_raw_secrets(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|e| e.secret.expose().to_string())
            .collect()
    }

    fn get(&self, scope: &str, server_id: &str) -> Option<&ServerIdentity> {
        let id = normalize_id(server_id);
        self.entries
            .iter()
            .find(|e| e.scope == scope && e.server_id == id)
    }

    fn get_mut(&mut self, scope: &str, server_id: &str) -> Option<&mut ServerIdentity> {
        let id = normalize_id(server_id);
        self.entries
            .iter_mut()
            .find(|e| e.scope == scope && e.server_id == id)
    }

    /// Secrets to try for an action, best first. An issued secret the mod does not have yet wins over the
    /// value from the XML: actions written before the mod received it carry none or an old one.
    /// `[None]` when no secret is known (the action then registers the savegame).
    pub fn candidates_for_action(
        &self,
        scope: &str,
        server_id: &str,
        from_mod: Option<&ServerSecret>,
    ) -> Vec<Option<ServerSecret>> {
        let known = self.get(scope, server_id);
        let own = known.map(|e| e.secret.clone());
        let prefer_own = known.is_some_and(|e| e.issued && !e.known_to_mod && !e.rejected);
        let mut ordered = if prefer_own {
            vec![own, from_mod.cloned()]
        } else {
            vec![from_mod.cloned(), own]
        };
        ordered.retain(Option::is_some);
        ordered.dedup();
        if ordered.is_empty() {
            ordered.push(None);
        }
        ordered
    }

    /// Secret for `GET /contracts/state`, `None` means: poll the board without `server_id`.
    pub fn secret_for_poll(&self, scope: &str, server_id: &str) -> Option<ServerSecret> {
        self.get(scope, server_id)
            .filter(|e| !e.rejected)
            .map(|e| e.secret.clone())
    }

    /// Secret that still has to be passed to the mod (`<serverIdentity>`).
    pub fn undelivered_secret(&self, scope: &str, server_id: &str) -> Option<ServerSecret> {
        self.get(scope, server_id)
            .filter(|e| e.issued && !e.known_to_mod && !e.rejected)
            .map(|e| e.secret.clone())
    }

    pub fn state(&self, scope: &str, server_id: &str) -> ServerIdentityState {
        let id = normalize_id(server_id);
        if self
            .rejected_without_secret
            .iter()
            .any(|(s, i)| s == scope && *i == id)
        {
            return ServerIdentityState::Rejected;
        }
        match self.get(scope, server_id) {
            Some(e) if e.rejected => ServerIdentityState::Rejected,
            Some(_) => ServerIdentityState::Known,
            None => ServerIdentityState::Unknown,
        }
    }

    /// Records a successful action. Returns `true` when the persisted part changed.
    pub fn record_accepted(
        &mut self,
        scope: &str,
        server_id: &str,
        used: Option<&ServerSecret>,
        from_mod: Option<&ServerSecret>,
        issued: Option<&ServerSecret>,
    ) -> bool {
        let before = self.issued_entries();
        let id = normalize_id(server_id);
        self.rejected_without_secret
            .retain(|(s, i)| !(s == scope && *i == id));

        if let Some(secret) = issued.or(used) {
            let was_issued = self.get(scope, &id).is_some_and(|e| e.issued);
            let next = ServerIdentity {
                scope: scope.to_string(),
                server_id: id.clone(),
                secret: secret.clone(),
                issued: issued.is_some() || was_issued,
                known_to_mod: from_mod == Some(secret),
                rejected: false,
            };
            match self.get_mut(scope, &id) {
                Some(entry) => *entry = next,
                None => {
                    self.entries.push(next);
                    if self.entries.len() > MAX_SERVER_IDENTITIES {
                        self.entries.remove(0);
                    }
                }
            }
        }
        before != self.issued_entries()
    }

    /// Records `403 SERVER_SECRET_INVALID`. The secret is kept as a fallback candidate. Returns `true`
    /// on the first rejection, so it is logged once.
    pub fn record_rejected(&mut self, scope: &str, server_id: &str) -> bool {
        if self.state(scope, server_id) == ServerIdentityState::Rejected {
            return false;
        }
        match self.get_mut(scope, server_id) {
            Some(entry) => entry.rejected = true,
            None => self
                .rejected_without_secret
                .push((scope.to_string(), normalize_id(server_id))),
        }
        true
    }
}
