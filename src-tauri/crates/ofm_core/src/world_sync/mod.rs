//! The live-sync skeleton for `world.db`: who may be asked, what would be asked for, and what a
//! provider's answer may become.
//!
//! The world database is built from sync and package imports, and the sync half needs a policy
//! before it needs a network: which providers are allowed at all, whether a route is worth asking
//! for, how a provider identity becomes a local one, and what a payload is allowed to write. This
//! module is that policy, in pure Rust. It links no HTTP client, opens no socket and reads no key
//! from disk on its own â€” [`SyncTransport`] is the seam a real client will plug into, and the only
//! implementation in the tree is [`DrySyncTransport`], which answers from a fixture.
//!
//! Three rules the rest of the module exists to keep:
//!
//! - **Dry by default.** A config that is not live, or has no usable key, plans a dry run. There is
//!   no code path here that turns an enabled-by-default flag into a request.
//! - **Identity is never merged by name.** A provider row maps to a world entity only through
//!   `(provider, provider_entity_id)`, and two canonical ids that disagree are reported, not
//!   reconciled â€” see [`canonical`].
//! - **Missing is never zero.** A statistic the provider did not report stays `None` all the way
//!   from the parsed payload to the record the database may store.

use std::fmt;

mod api_football;
mod canonical;
mod config;
mod policy;

pub use api_football::{
    API_FOOTBALL_BASE_URL, API_KEY_HEADER, ApiFootballAdapter, ClubRecord, CompetitionRecord,
    DrySyncTransport, ENTITY_KIND_CLUB, ENTITY_KIND_COMPETITION, ENTITY_KIND_PLAYER,
    EntityLinkRecord, FixtureBatch, PlannedStep, PlayerRecord, SyncExecution, SyncOrchestrator,
    SyncPlan, SyncRequest, SyncResponse, SyncRoute, SyncStepOutcome, SyncStepResult, SyncTransport,
    provisional_world_id,
};
pub use canonical::{
    CanonicalConflict, CanonicalLink, CanonicalResolution, EntityLinkCandidate, resolve_links,
};
pub use config::{ApiKeyHandle, LiveBlockReason, ProviderKind, SyncMode, WorldSyncConfig};
pub use policy::{
    RouteFreshness, SyncRequestPlan, default_retention, expires_at_from, plan_request,
    prune_cutoff, should_prune,
};

/// What can go wrong while planning, parsing or resolving a sync.
///
/// These are not translation keys: the reader is the sync screen's own diagnostics and the tests,
/// never a rendered player string. A command that surfaces one is responsible for mapping it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldSyncError {
    /// The provider is outside the legal boundary this project keeps: its terms forbid automated
    /// collection, so the code refuses the name instead of letting a caller opt in.
    ProviderNotAllowed { provider: String },
    /// A provider name nothing in the tree implements.
    UnknownProvider { provider: String },
    /// A field identity is keyed on was blank; a blank key would silently merge unrelated rows.
    BlankField {
        field: &'static str,
        context: String,
    },
    /// A link confidence outside the `0..=100` the canonical contract allows.
    ConfidenceOutOfRange { confidence: u16 },
    /// A payload was not the shape the adapter expects, or named a route with no dry fixture.
    Payload(String),
}

impl fmt::Display for WorldSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderNotAllowed { provider } => write!(
                formatter,
                "provider `{provider}` is not allowed: its terms forbid automated collection"
            ),
            Self::BlankField { field, context } => {
                write!(formatter, "{field} is blank in {context}")
            }
            Self::ConfidenceOutOfRange { confidence } => write!(
                formatter,
                "a link confidence of {confidence} is outside 0..=100"
            ),
            Self::UnknownProvider { provider } => {
                write!(
                    formatter,
                    "no sync adapter implements provider `{provider}`"
                )
            }
            Self::Payload(error) => write!(formatter, "provider payload: {error}"),
        }
    }
}

impl std::error::Error for WorldSyncError {}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("a representable instant")
    }

    /// Given a default (dry) config, the embedded fixtures and no key
    /// When a run plans, fetches the fixtures route and resolves every identity it found
    /// Then nothing was spent, the plan says dry, and every club and competition resolves to one
    ///      canonical link with no conflict.
    #[test]
    fn a_first_dry_run_plans_fetches_and_resolves_without_a_conflict() {
        let execution = SyncOrchestrator::new(WorldSyncConfig::default())
            .run(
                &[SyncRoute::first("fixtures", Vec::new())],
                at(1_000),
                &DrySyncTransport::new(),
            )
            .expect("a dry run needs no network and no key");

        assert_eq!(execution.plan.mode, SyncMode::DryRun);
        assert_eq!(execution.plan.provider, ProviderKind::ApiFootball);

        let body = execution
            .body("fixtures")
            .expect("a dry run keeps the fetched body");
        let batch = ApiFootballAdapter::new()
            .parse_fixtures(body)
            .expect("the fetched fixture parses");
        assert_eq!(batch.links.len(), 6, "two competitions and four clubs");

        let candidates: Vec<EntityLinkCandidate> = batch
            .links
            .iter()
            .map(|link| {
                EntityLinkCandidate::new(
                    link.provider.as_str(),
                    link.provider_entity_id.as_str(),
                    link.entity_kind.as_str(),
                    link.entity_id.as_str(),
                    90,
                )
                .observed_on("2026-08-15")
            })
            .collect();
        let resolution =
            resolve_links(&candidates).expect("every implied identity is keyed by provider");

        assert!(resolution.conflicts.is_empty());
        assert_eq!(resolution.accepted.len(), 6);
        assert!(
            resolution.accepted.iter().all(|link| {
                link.confidence == 90 && link.as_of.as_deref() == Some("2026-08-15")
            })
        );
    }

    /// Given a config asking for live sync with neither the switch nor a key
    /// When the orchestrator plans a route
    /// Then the plan reports dry and the request it shows carries no key header.
    #[test]
    fn a_plan_cannot_claim_live_without_the_switch_and_a_key() {
        let plan = SyncOrchestrator::new(WorldSyncConfig::default().requesting(SyncMode::Live))
            .plan(&[SyncRoute::first("players", Vec::new())], at(1_000))
            .expect("a valid base url");

        assert_eq!(plan.mode, SyncMode::DryRun);
        let request = plan.steps[0].request.as_ref().expect("a first run fetches");
        assert_eq!(request.header(API_KEY_HEADER), None);
    }

    /// Given every error the module can return
    /// When each is displayed
    /// Then it names the thing that failed, so a diagnostics screen has something to say.
    #[test]
    fn every_error_variant_displays_a_reason() {
        let errors = [
            WorldSyncError::ProviderNotAllowed {
                provider: String::from("sofascore"),
            },
            WorldSyncError::UnknownProvider {
                provider: String::from("example-feed"),
            },
            WorldSyncError::BlankField {
                field: "provider",
                context: String::from("the link proposal for provider id `874`"),
            },
            WorldSyncError::ConfidenceOutOfRange { confidence: 101 },
            WorldSyncError::Payload(String::from("expected value at line 1")),
        ];
        for error in errors {
            let message = error.to_string();
            assert!(!message.is_empty());
            assert!(message.len() > 20, "{message}");
        }
        assert!(
            WorldSyncError::ProviderNotAllowed {
                provider: String::from("transfermarkt")
            }
            .to_string()
            .contains("transfermarkt")
        );
    }
}
