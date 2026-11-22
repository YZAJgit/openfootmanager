//! Who the world database may be synced from, and whether a run is live at all.
//!
//! The config is deliberately dull: a provider, a mode the caller *asks* for, and a handle to the
//! user's own key. Whether a run is actually live is derived, never set — it needs both a live
//! request and a key that resolves to something, so a missing key, a blank key or a build with live
//! sync off all end in a dry run rather than in a request. That is the fail-closed direction: the
//! failure mode of forgetting to configure something is that nothing is spent.

use std::fmt;

use super::WorldSyncError;

/// The providers a world can be synced from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// API-Football. Bring your own key; the free tier is enough for a dry plan.
    ApiFootball,
    /// A package or file a person imported. No network, no key.
    ManualImport,
    /// The deterministic local fixture the dry transport answers from.
    LocalFixture,
}

/// Provider codes the project refuses on legal grounds, not technical ones: their terms prohibit
/// automated collection, so supporting them would make every sync a licence violation.
const BANNED_PROVIDERS: [&str; 2] = ["transfermarkt", "sofascore"];

impl ProviderKind {
    /// The stable code a sync run stores in `world_sync_runs.provider`.
    pub fn code(self) -> &'static str {
        match self {
            Self::ApiFootball => "api-football",
            Self::ManualImport => "manual-import",
            Self::LocalFixture => "local-fixture",
        }
    }

    /// Read a stored code back, refusing the names the project may not use.
    ///
    /// Case and separators are forgiven (`API_Football` is API-Football), because the code travels
    /// through a settings file and an env var. The ban is checked first and is not forgiven: a
    /// caller that spells a banned provider differently gets the legal refusal, not "unknown".
    pub fn parse(code: &str) -> Result<Self, WorldSyncError> {
        let normalized = code.trim().to_ascii_lowercase().replace('_', "-");
        let compact: String = normalized
            .chars()
            .filter(|ch| !matches!(ch, '-' | '_' | '.' | ' ' | '\t'))
            .collect();
        if BANNED_PROVIDERS
            .iter()
            .any(|banned| compact.contains(banned))
        {
            return Err(WorldSyncError::ProviderNotAllowed {
                provider: normalized,
            });
        }
        match normalized.as_str() {
            "api-football" | "apifootball" => Ok(Self::ApiFootball),
            "manual-import" | "manualimport" => Ok(Self::ManualImport),
            "local-fixture" | "localfixture" => Ok(Self::LocalFixture),
            other => Err(WorldSyncError::UnknownProvider {
                provider: other.to_string(),
            }),
        }
    }

    /// Whether this provider can be asked anything over a network at all.
    pub fn is_remote(self) -> bool {
        matches!(self, Self::ApiFootball)
    }
}

/// How much a run is allowed to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SyncMode {
    /// Plan and parse, spend nothing. The default.
    #[default]
    DryRun,
    /// Ask the provider. Only reachable through [`WorldSyncConfig::effective_mode`].
    Live,
}

impl SyncMode {
    /// The stable code a sync run stores in `world_sync_runs.mode`.
    pub fn code(self) -> &'static str {
        match self {
            Self::DryRun => "dry-run",
            Self::Live => "live",
        }
    }
}

/// Why a requested live run was downgraded to a dry one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveBlockReason {
    /// The build or the user has not enabled live sync.
    LiveDisabled,
    /// No key handle was configured.
    MissingKey,
    /// The handle resolved to nothing: an unset or blank variable, or a blank supplied key.
    BlankKey,
}

impl LiveBlockReason {
    /// A code a diagnostics screen can show without inventing prose per locale.
    pub fn code(self) -> &'static str {
        match self {
            Self::LiveDisabled => "live-disabled",
            Self::MissingKey => "missing-key",
            Self::BlankKey => "blank-key",
        }
    }
}

/// Where the user's own provider key lives.
///
/// The config holds this handle, never a resolved key in a printed field: [`ApiKeyHandle`]'s
/// `Debug` prints the variable's name and redacts the key itself, so a log line, an error or a
/// panic message cannot leak it. The key leaves the handle only through
/// [`WorldSyncConfig::api_key`], which a request builder calls at the moment it builds a header.
#[derive(Clone, PartialEq, Eq)]
pub enum ApiKeyHandle {
    /// The name of the environment variable the key is read from.
    Environment { variable: String },
    /// A key the caller already holds — a keychain read, or a test.
    Supplied { key: String },
}

impl ApiKeyHandle {
    /// Read the key from `variable` when it is needed.
    pub fn environment(variable: impl Into<String>) -> Self {
        Self::Environment {
            variable: variable.into(),
        }
    }

    /// A key that is already in hand.
    pub fn supplied(key: impl Into<String>) -> Self {
        Self::Supplied { key: key.into() }
    }

    /// The key, or `None` when the handle is unset or blank.
    ///
    /// Blank counts as missing: a variable set to `""` or spaces is a half-finished setup, and
    /// sending it would only spend a failed request.
    pub fn resolve(&self) -> Option<String> {
        let key = match self {
            Self::Environment { variable } => std::env::var(variable).ok()?,
            Self::Supplied { key } => key.clone(),
        };
        let trimmed = key.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

impl fmt::Debug for ApiKeyHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment { variable } => formatter
                .debug_tuple("Environment")
                .field(variable)
                .finish(),
            Self::Supplied { .. } => formatter.write_str("Supplied(<redacted>)"),
        }
    }
}

/// Everything one sync run needs to know before it plans a single request.
#[derive(Clone, PartialEq, Eq)]
pub struct WorldSyncConfig {
    pub provider: ProviderKind,
    /// What the caller asked for. [`Self::effective_mode`] decides what it gets.
    pub requested_mode: SyncMode,
    /// The bring-your-own key handle. Never printed.
    pub key: Option<ApiKeyHandle>,
    /// The switch that makes live calls possible at all in this build.
    pub live_enabled: bool,
}

impl Default for WorldSyncConfig {
    /// API-Football, dry, no key, live off: the state a fresh install is in.
    fn default() -> Self {
        Self {
            provider: ProviderKind::ApiFootball,
            requested_mode: SyncMode::DryRun,
            key: None,
            live_enabled: false,
        }
    }
}

impl WorldSyncConfig {
    /// A dry configuration for `provider`.
    pub fn new(provider: ProviderKind) -> Self {
        Self {
            provider,
            ..Self::default()
        }
    }

    /// Attach the user's key handle.
    pub fn with_key(mut self, key: ApiKeyHandle) -> Self {
        self.key = Some(key);
        self
    }

    /// Ask for a mode. Asking is not permission — see [`Self::effective_mode`].
    pub fn requesting(mut self, mode: SyncMode) -> Self {
        self.requested_mode = mode;
        self
    }

    /// Turn the live switch on or off.
    pub fn with_live_enabled(mut self, live_enabled: bool) -> Self {
        self.live_enabled = live_enabled;
        self
    }

    /// Why this config cannot go live, or `None` when it can.
    pub fn live_block_reason(&self) -> Option<LiveBlockReason> {
        if !self.live_enabled {
            return Some(LiveBlockReason::LiveDisabled);
        }
        match &self.key {
            None => Some(LiveBlockReason::MissingKey),
            Some(handle) if handle.resolve().is_none() => Some(LiveBlockReason::BlankKey),
            Some(_) => None,
        }
    }

    /// The mode a run actually gets: live only when it was asked for *and* nothing blocks it.
    pub fn effective_mode(&self) -> SyncMode {
        match (self.requested_mode, self.live_block_reason()) {
            (SyncMode::Live, None) => SyncMode::Live,
            (SyncMode::Live, Some(_)) | (SyncMode::DryRun, _) => SyncMode::DryRun,
        }
    }

    /// The resolved key, for the one place that builds a request header.
    pub fn api_key(&self) -> Option<String> {
        self.key.as_ref().and_then(ApiKeyHandle::resolve)
    }
}

/// Redacted on purpose: this type is logged, returned from a settings command, and named in
/// diagnostics, and none of those may ever hold the user's key.
impl fmt::Debug for WorldSyncConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorldSyncConfig")
            .field("provider", &self.provider)
            .field("requested_mode", &self.requested_mode)
            .field("effective_mode", &self.effective_mode())
            .field("key", &self.key)
            .field("live_enabled", &self.live_enabled)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "sk-live-9f8e7d6c5b4a";

    /// Given a config with a usable key but live sync switched off
    /// When its mode is derived
    /// Then it stays dry and names the switch that blocked it.
    #[test]
    fn a_key_without_the_live_switch_stays_dry() {
        let config = WorldSyncConfig::new(ProviderKind::ApiFootball)
            .with_key(ApiKeyHandle::supplied(SECRET))
            .requesting(SyncMode::Live);

        assert_eq!(config.effective_mode(), SyncMode::DryRun);
        assert_eq!(
            config.live_block_reason(),
            Some(LiveBlockReason::LiveDisabled)
        );
    }

    /// Given live sync switched on and no key configured
    /// When its mode is derived
    /// Then it stays dry rather than asking the provider with no identity.
    #[test]
    fn a_live_switch_without_a_key_stays_dry() {
        let config = WorldSyncConfig::new(ProviderKind::ApiFootball)
            .requesting(SyncMode::Live)
            .with_live_enabled(true);

        assert_eq!(config.effective_mode(), SyncMode::DryRun);
        assert_eq!(
            config.live_block_reason(),
            Some(LiveBlockReason::MissingKey)
        );
        assert_eq!(config.api_key(), None);
    }

    /// Given a key that is blank or only spaces
    /// When its mode is derived
    /// Then the run stays dry: a half-finished setup spends nothing.
    #[test]
    fn a_blank_key_never_turns_a_run_live() {
        for blank in ["", "   ", "\t\n"] {
            let config = WorldSyncConfig::new(ProviderKind::ApiFootball)
                .with_key(ApiKeyHandle::supplied(blank))
                .requesting(SyncMode::Live)
                .with_live_enabled(true);

            assert_eq!(config.effective_mode(), SyncMode::DryRun, "{blank:?}");
            assert_eq!(config.live_block_reason(), Some(LiveBlockReason::BlankKey));
            assert_eq!(config.api_key(), None);
        }
    }

    /// Given live sync on, a handle that resolves to a key, and a live request
    /// When its mode is derived
    /// Then it is live and the key resolves with surrounding spaces removed.
    #[test]
    fn a_complete_live_setup_is_live() {
        let config = WorldSyncConfig::new(ProviderKind::ApiFootball)
            .with_key(ApiKeyHandle::supplied(format!("  {SECRET}  ")))
            .requesting(SyncMode::Live)
            .with_live_enabled(true);

        assert_eq!(config.effective_mode(), SyncMode::Live);
        assert_eq!(config.live_block_reason(), None);
        assert_eq!(config.api_key().as_deref(), Some(SECRET));
    }

    /// Given a config holding a secret
    /// When it is formatted with `Debug`, directly and inside the handle
    /// Then the secret appears nowhere and the redaction does.
    #[test]
    fn debug_output_never_carries_the_api_key() {
        let config = WorldSyncConfig::new(ProviderKind::ApiFootball)
            .with_key(ApiKeyHandle::supplied(SECRET))
            .requesting(SyncMode::Live)
            .with_live_enabled(true);
        let config_debug = format!("{config:?}");
        let handle_debug = format!("{:?}", config.key);

        assert!(!config_debug.contains(SECRET), "{config_debug}");
        assert!(config_debug.contains("redacted"), "{config_debug}");
        assert_eq!(handle_debug, "Some(Supplied(<redacted>))");
        assert!(
            config_debug.contains("effective_mode: Live"),
            "{config_debug}"
        );
    }

    /// Given a key handle pointing at an environment variable nothing sets
    /// When it is resolved
    /// Then it yields nothing instead of an empty string.
    #[test]
    fn an_unset_environment_variable_resolves_to_nothing() {
        let handle = ApiKeyHandle::environment("OFM_TEST_UNSET_API_FOOTBALL_KEY");
        assert_eq!(handle.resolve(), None);
        // The variable's name is not a secret; the key it would hold is.
        assert!(format!("{handle:?}").contains("OFM_TEST_UNSET_API_FOOTBALL_KEY"));
    }

    /// Given a banned provider name in any casing or separator style
    /// When it is parsed
    /// Then it is refused on the legal boundary, not reported as merely unknown.
    #[test]
    fn a_banned_provider_is_refused_with_the_legal_reason() {
        for banned in [
            "transfermarkt",
            "TransferMarkt",
            "transfer-markt",
            "transfermarkt.com",
            "SOFASCORE",
            " sofascore ",
            "sofa_score",
            "sofa-score",
        ] {
            let error = ProviderKind::parse(banned).expect_err("a banned provider must be refused");
            assert!(
                matches!(error, WorldSyncError::ProviderNotAllowed { .. }),
                "{banned} gave {error:?}"
            );
            assert!(error.to_string().contains("terms"), "{error}");
        }
    }

    /// Given a name no adapter implements
    /// When it is parsed
    /// Then it is reported as unknown rather than banned.
    #[test]
    fn an_unimplemented_provider_is_unknown_not_banned() {
        let error =
            ProviderKind::parse("example-feed").expect_err("an unknown provider must be refused");
        assert_eq!(
            error,
            WorldSyncError::UnknownProvider {
                provider: String::from("example-feed")
            }
        );
    }

    /// Given each provider code
    /// When it is written and read back, including through the separator variations a settings
    /// file may carry
    /// Then it round trips to the same provider.
    #[test]
    fn provider_codes_round_trip() {
        for provider in [
            ProviderKind::ApiFootball,
            ProviderKind::ManualImport,
            ProviderKind::LocalFixture,
        ] {
            assert_eq!(ProviderKind::parse(provider.code()), Ok(provider));
        }
        assert_eq!(
            ProviderKind::parse("API_Football"),
            Ok(ProviderKind::ApiFootball)
        );
        assert_eq!(
            ProviderKind::parse(" apifootball "),
            Ok(ProviderKind::ApiFootball)
        );
        assert!(ProviderKind::ApiFootball.is_remote());
        assert!(!ProviderKind::ManualImport.is_remote());
    }

    /// Given a fresh config
    /// When it is built with no arguments
    /// Then it is dry, keyless and on the only remote provider.
    #[test]
    fn a_default_config_is_a_dry_api_football_plan() {
        let config = WorldSyncConfig::default();
        assert_eq!(config.provider, ProviderKind::ApiFootball);
        assert_eq!(config.requested_mode, SyncMode::DryRun);
        assert_eq!(config.effective_mode(), SyncMode::DryRun);
        assert_eq!(config.key, None);
        assert!(!config.live_enabled);
        assert_eq!(SyncMode::default(), SyncMode::DryRun);
        assert_eq!(SyncMode::DryRun.code(), "dry-run");
        assert_eq!(LiveBlockReason::BlankKey.code(), "blank-key");
    }
}
