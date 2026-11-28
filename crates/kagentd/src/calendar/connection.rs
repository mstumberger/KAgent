//! The connection of the hub (calendar.md section 1): the client, the sign-in and its
//! callback, the access token and its refresh, the break (`invalid_grant`) and the
//! disconnect, and the settings the module saves. The state itself is in `mod.rs`.

use serde_json::{json, Value};

use kagent_core::protocol::AgentEvent;

use super::oauth::{self, Pending, TokenError};
use super::{state_of, CalError, Calendar, ConnState, Inner, Secrets, RECONNECT_TEXT};

impl Calendar {
    /// Run a change of the inner state and tell the page when the connection state moved.
    pub(super) fn change<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        let (out, before, after) = {
            let mut inner = self.inner.lock().unwrap();
            let before = state_of(&inner, self.now()).0;
            let out = f(&mut inner);
            (out, before, state_of(&inner, self.now()).0)
        };
        if before != after {
            let _ = self.events.send(AgentEvent::CalendarChanged { state: after.name().into() });
        }
        out
    }

    pub(super) async fn save_secrets(&self, secrets: &Secrets) -> anyhow::Result<()> {
        if *secrets == Secrets::default() {
            self.store.clear().await
        } else {
            self.store.set(&serde_json::to_string(secrets)?).await
        }
    }

    /// `PUT /api/calendar/client`: the client id goes to the settings, the secret to the
    /// keyring (`None` or empty keeps the one stored).
    pub async fn set_client(&self, client_id: &str, client_secret: Option<&str>) -> Result<(), CalError> {
        let id = client_id.trim();
        if id.is_empty() || id.len() > 200 || id.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(CalError::Setup("the client id is empty or has spaces: paste it from the Google Cloud console as it is".into()));
        }
        let secrets = {
            let mut inner = self.inner.lock().unwrap();
            if let Some(secret) = client_secret.map(str::trim).filter(|s| !s.is_empty()) {
                if secret.len() > 200 || secret.chars().any(|c| c.is_whitespace() || c.is_control()) {
                    return Err(CalError::Setup("the client secret has spaces: paste it from the Google Cloud console as it is".into()));
                }
                inner.secrets.client_secret = secret.to_string();
            }
            inner.secrets.clone()
        };
        self.save_secrets(&secrets).await.map_err(|e| CalError::Upstream(format!("the client could not be stored: {e:#}")))?;
        self.patch_settings(json!({"calendar": {"clientId": id}})).map_err(CalError::Setup)
    }

    /// `POST /api/calendar/connect`: the address the owner opens at Google.
    pub fn connect(&self) -> Result<String, CalError> {
        let id = self.config().client_id;
        let mut inner = self.inner.lock().unwrap();
        if id.is_empty() || inner.secrets.client_secret.is_empty() {
            return Err(CalError::Setup("paste the client id and the client secret of your Google Cloud OAuth client first (Settings -> Calendar)".into()));
        }
        let before = state_of(&inner, self.now()).0;
        if matches!(inner.sticky, Some((ConnState::Error, _))) {
            inner.sticky = None;
        }
        let (state, challenge) = inner.pending.begin(self.now());
        let url = oauth::auth_url(&self.hosts, &id, &self.redirect, &state, &challenge);
        let after = state_of(&inner, self.now()).0;
        drop(inner);
        if before != after {
            let _ = self.events.send(AgentEvent::CalendarChanged { state: after.name().into() });
        }
        Ok(url)
    }

    /// The callback of Google (calendar.md section 1): a known, unused, recent `state`, then
    /// the code for tokens.
    pub async fn callback(&self, code: &str, state: &str) -> Result<(), CalError> {
        let (verifier, client_secret) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(verifier) = inner.pending.take(state, self.now()) else {
                return Err(CalError::Refused("this sign-in link is unknown, used or older than 10 minutes: press Connect again".into()));
            };
            (verifier, inner.secrets.client_secret.clone())
        };
        let granted = oauth::exchange(&self.http, &self.hosts, &self.config().client_id, &client_secret, code, &verifier, &self.redirect).await.map_err(|e| {
            self.change(|inner| inner.sticky = Some((ConnState::Error, e.to_string())));
            CalError::Upstream(e.to_string())
        })?;
        let Some(refresh) = granted.refresh else {
            let why = "Google gave no refresh token: remove KAgent at myaccount.google.com/permissions and connect again".to_string();
            self.change(|inner| inner.sticky = Some((ConnState::Error, why.clone())));
            return Err(CalError::Upstream(why));
        };
        let ends = self.now() + chrono::Duration::seconds(granted.expires_in);
        let secrets = self.change(|inner| {
            inner.secrets.refresh_token = refresh;
            inner.access = Some((granted.access, ends));
            inner.sticky = None;
            inner.told_reconnect = false;
            inner.secrets.clone()
        });
        self.save_secrets(&secrets).await.map_err(|e| CalError::Upstream(format!("the connection could not be stored: {e:#}")))?;
        tracing::info!("calendar: connected");
        self.first_connection();
        Ok(())
    }

    /// The first connection switches the module on and puts its tools where they are used:
    /// the tool groups of the chat and of the voice (calendar.md section 3: "default on once
    /// connected"). Later connections leave the owner's choices alone.
    fn first_connection(&self) {
        let (was_on, groups, voice) = {
            let s = self.settings.read().unwrap();
            (s.calendar_config().enabled, s.tool_groups.clone(), s.section::<crate::sections::VoiceSection>("voice").unwrap_or_default().tool_groups)
        };
        if was_on {
            return;
        }
        let with = |mut list: Vec<String>| {
            if !list.iter().any(|g| g == "calendar") {
                list.push("calendar".into());
            }
            list
        };
        let patch = json!({"calendar": {"enabled": true}, "toolGroups": with(groups), "voice": {"toolGroups": with(voice)}});
        if let Err(e) = self.patch_settings(patch) {
            tracing::warn!("calendar: the settings could not be switched on: {e}");
        }
    }

    /// An access token that is good for at least a minute, refreshed when needed.
    pub async fn access_token(&self) -> Result<String, CalError> {
        if let Some(token) = self.cached_access() {
            return Ok(token);
        }
        let _one = self.refreshing.lock().await;
        if let Some(token) = self.cached_access() {
            return Ok(token);
        }
        let (client_secret, refresh) = {
            let inner = self.inner.lock().unwrap();
            (inner.secrets.client_secret.clone(), inner.secrets.refresh_token.clone())
        };
        if refresh.is_empty() {
            return Err(CalError::NotConnected("Google Calendar is not connected: the owner connects it in Settings -> Calendar.".into()));
        }
        match oauth::refresh(&self.http, &self.hosts, &self.config().client_id, &client_secret, &refresh).await {
            Ok(granted) => {
                let ends = self.now() + chrono::Duration::seconds(granted.expires_in);
                self.change(|inner| {
                    inner.access = Some((granted.access.clone(), ends));
                    if matches!(inner.sticky, Some((ConnState::Error, _))) {
                        inner.sticky = None;
                    }
                });
                Ok(granted.access)
            }
            Err(TokenError::InvalidGrant) => {
                self.break_connection();
                Err(CalError::NotConnected(RECONNECT_TEXT.into()))
            }
            Err(TokenError::Other(why)) => {
                self.change(|inner| inner.sticky = Some((ConnState::Error, why.clone())));
                Err(CalError::Upstream(why))
            }
        }
    }

    fn cached_access(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        let (token, ends) = inner.access.as_ref()?;
        (*ends - self.now() > chrono::Duration::seconds(60)).then(|| token.clone())
    }

    /// Google said 401 to this token: the next call refreshes.
    pub fn forget_access(&self) {
        self.inner.lock().unwrap().access = None;
    }

    /// `invalid_grant`: the state is `reconnect`, one notice, nothing is deleted.
    pub(crate) fn break_connection(&self) {
        let tell = self.change(|inner| {
            inner.access = None;
            inner.sticky = Some((ConnState::Reconnect, RECONNECT_TEXT.into()));
            !std::mem::replace(&mut inner.told_reconnect, true)
        });
        if tell {
            tracing::warn!("calendar: Google no longer accepts the connection");
            let _ = self.events.send(AgentEvent::keyed_notice("warn", "Google Calendar needs to be connected again (Settings -> Calendar).", "calendar:reconnect"));
        }
    }

    /// `POST /api/calendar/disconnect`: the token is revoked at Google and deleted here; the
    /// client and the settings stay, so Connect works again without pasting anything.
    pub async fn disconnect(&self) -> Result<(), CalError> {
        let refresh = self.inner.lock().unwrap().secrets.refresh_token.clone();
        if !refresh.is_empty() {
            if let Err(e) = oauth::revoke(&self.http, &self.hosts, &refresh).await {
                tracing::warn!("calendar: the token could not be revoked at Google: {e}");
            }
        }
        let secrets = self.change(|inner| {
            inner.secrets.refresh_token.clear();
            inner.secrets.account.clear();
            inner.access = None;
            inner.sticky = None;
            inner.told_reconnect = false;
            inner.pending = Pending::default();
            inner.secrets.clone()
        });
        self.save_secrets(&secrets).await.map_err(|e| CalError::Upstream(format!("the connection could not be deleted: {e:#}")))?;
        tracing::info!("calendar: disconnected");
        Ok(())
    }

    /// Save a settings patch like `PUT /api/settings` does and tell the open pages.
    pub(crate) fn patch_settings(&self, patch: Value) -> Result<(), String> {
        let settings = {
            let mut s = self.settings.write().unwrap();
            s.patch(patch)?;
            s.save(&self.settings_path).map_err(|e| format!("{e:#}"))?;
            s.clone()
        };
        let _ = self.events.send(AgentEvent::SettingsChanged { settings: serde_json::to_value(&settings).unwrap_or(Value::Null) });
        Ok(())
    }

    pub(crate) fn set_account(&self, account: &str) {
        self.inner.lock().unwrap().secrets.account = account.to_string();
    }
}
