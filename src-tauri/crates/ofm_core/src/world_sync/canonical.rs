//! Turning a provider's identity into a world entity â€” never by name.
//!
//! Two databases describing the same football world always disagree somewhere: a club is spelled
//! differently, a player's second surname is missing, two players share a name exactly. Matching on
//! that is how a career ends up with a striker's goals on a reserve team-mate, so this module has no
//! name field to match on. A link exists only as `(provider, provider_entity_id) -> world id`, and
//! when one provider identity proposes two world entities the answer is a reported conflict for a
//! person to settle, not a winner chosen by order or confidence.
//!
//! Confidence is `0..=100` â€” a percentage, because that is what a reviewer reasons about â€” and it
//! is stored by `db::world_db::sync_store` in the `0.0..=1.0` unit `entity_links` declares.

use std::collections::{BTreeMap, BTreeSet};

use super::WorldSyncError;

/// A provider identity proposing (or confirming) a canonical world entity.
///
/// Deliberately carries no name: identity is never merged by name, so there is nowhere to pass one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLinkCandidate {
    /// The provider code, e.g. `api-football`.
    pub provider: String,
    /// The provider's own id for the entity, e.g. `874`.
    pub provider_entity_id: String,
    /// The local entity's kind: `player`, `club`, `competition`.
    pub entity_kind: String,
    /// The canonical world id this provider identity maps to.
    pub entity_id: String,
    /// `0..=100`. How sure the proposal is; a missing guess is not a zero-confidence link, it is no
    /// link at all, so there is no `Option` here â€” a caller that has no confidence does not propose.
    pub confidence: u8,
    /// `YYYY-MM-DD`, when the link was observed. `None` is "nobody recorded a date".
    pub as_of: Option<String>,
}

impl EntityLinkCandidate {
    /// A proposal with no observation date.
    pub fn new(
        provider: impl Into<String>,
        provider_entity_id: impl Into<String>,
        entity_kind: impl Into<String>,
        entity_id: impl Into<String>,
        confidence: u8,
    ) -> Self {
        Self {
            provider: provider.into(),
            provider_entity_id: provider_entity_id.into(),
            entity_kind: entity_kind.into(),
            entity_id: entity_id.into(),
            confidence,
            as_of: None,
        }
    }

    /// The same proposal, with the date it was observed.
    pub fn observed_on(mut self, as_of: impl Into<String>) -> Self {
        self.as_of = Some(as_of.into());
        self
    }
}

/// One provider identity resolved to one world entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalLink {
    pub provider: String,
    pub provider_entity_id: String,
    pub entity_kind: String,
    pub entity_id: String,
    pub confidence: u8,
    pub as_of: Option<String>,
}

/// A provider identity its own proposals disagree about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalConflict {
    pub provider: String,
    pub provider_entity_id: String,
    pub entity_kind: String,
    /// Every world id proposed for this identity, sorted, so the report is stable.
    pub entity_ids: Vec<String>,
}

/// What a batch of proposals resolved to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CanonicalResolution {
    /// One link per provider identity that agreed with itself, in key order.
    pub accepted: Vec<CanonicalLink>,
    /// Identities that proposed more than one world entity. Never merged; a person decides.
    pub conflicts: Vec<CanonicalConflict>,
}

/// Resolve a batch of proposals.
///
/// Proposals for one `(provider, provider_entity_id, entity_kind)` are folded together: the same
/// world id twice is a confirmation (the highest confidence and its date win), two different world
/// ids are a conflict. Every proposal is validated first, so a batch with a blank id writes nothing
/// at all rather than half of itself.
pub fn resolve_links(
    candidates: &[EntityLinkCandidate],
) -> Result<CanonicalResolution, WorldSyncError> {
    let mut groups: BTreeMap<(String, String, String), Proposals> = BTreeMap::new();
    for candidate in candidates {
        validate(candidate)?;
        let key = (
            trimmed(&candidate.provider),
            trimmed(&candidate.provider_entity_id),
            trimmed(&candidate.entity_kind),
        );
        groups.entry(key).or_default().observe(candidate);
    }
    Ok(fold(groups))
}

/// The proposals seen for one provider identity.
#[derive(Default)]
struct Proposals {
    entity_ids: BTreeSet<String>,
    /// The strongest proposal so far; its confidence and date are the ones a link carries.
    strongest: Option<(u8, String, Option<String>)>,
}

impl Proposals {
    fn observe(&mut self, candidate: &EntityLinkCandidate) {
        let entity_id = trimmed(&candidate.entity_id);
        self.entity_ids.insert(entity_id.clone());
        let as_of = candidate
            .as_of
            .as_deref()
            .map(str::trim)
            .map(str::to_string);
        let stronger = self
            .strongest
            .as_ref()
            .is_none_or(|(confidence, _, existing_as_of)| {
                (candidate.confidence, &as_of) > (*confidence, existing_as_of)
            });
        if stronger {
            self.strongest = Some((candidate.confidence, entity_id, as_of));
        }
    }
}

fn fold(groups: BTreeMap<(String, String, String), Proposals>) -> CanonicalResolution {
    let mut resolution = CanonicalResolution::default();
    for ((provider, provider_entity_id, entity_kind), proposals) in groups {
        if proposals.entity_ids.len() > 1 {
            resolution.conflicts.push(CanonicalConflict {
                provider,
                provider_entity_id,
                entity_kind,
                entity_ids: proposals.entity_ids.into_iter().collect(),
            });
            continue;
        }
        let Some((confidence, entity_id, as_of)) = proposals.strongest else {
            continue;
        };
        resolution.accepted.push(CanonicalLink {
            provider,
            provider_entity_id,
            entity_kind,
            entity_id,
            confidence,
            as_of: as_of.filter(|date| !date.is_empty()),
        });
    }
    resolution
}

fn validate(candidate: &EntityLinkCandidate) -> Result<(), WorldSyncError> {
    let context = format!(
        "the link proposal for provider id `{}`",
        trimmed(&candidate.provider_entity_id)
    );
    for (field, value) in [
        ("provider", candidate.provider.as_str()),
        ("provider entity id", candidate.provider_entity_id.as_str()),
        ("entity kind", candidate.entity_kind.as_str()),
        ("entity id", candidate.entity_id.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(WorldSyncError::BlankField {
                field,
                context: context.clone(),
            });
        }
    }
    if candidate.confidence > 100 {
        return Err(WorldSyncError::ConfidenceOutOfRange {
            confidence: u16::from(candidate.confidence),
        });
    }
    Ok(())
}

fn trimmed(value: &str) -> String {
    value.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(provider_id: &str, entity_id: &str, confidence: u8) -> EntityLinkCandidate {
        EntityLinkCandidate::new("api-football", provider_id, "player", entity_id, confidence)
    }

    /// Given one provider identity proposed twice with the same world id
    /// When the batch resolves
    /// Then it is one link, carrying the stronger confidence and its date.
    #[test]
    fn a_repeated_proposal_is_one_link_with_the_strongest_confidence() {
        let resolution = resolve_links(&[
            candidate("874", "player-1", 40),
            candidate("874", "player-1", 90).observed_on("2026-11-20"),
            candidate("874", "player-1", 60),
        ])
        .expect("a batch of valid proposals resolves");

        assert!(resolution.conflicts.is_empty());
        assert_eq!(resolution.accepted.len(), 1);
        let link = &resolution.accepted[0];
        assert_eq!(link.entity_id, "player-1");
        assert_eq!(link.confidence, 90);
        assert_eq!(link.as_of.as_deref(), Some("2026-11-20"));
    }

    /// Given one provider identity proposing two different world ids
    /// When the batch resolves
    /// Then it is a conflict naming both ids, and no link is accepted for that identity.
    #[test]
    fn two_canonical_ids_for_one_provider_identity_are_a_conflict() {
        let resolution = resolve_links(&[
            candidate("874", "player-2", 80),
            candidate("874", "player-1", 80),
            candidate("875", "player-3", 70),
        ])
        .expect("conflicting proposals are still valid input");

        assert_eq!(resolution.conflicts.len(), 1);
        let conflict = &resolution.conflicts[0];
        assert_eq!(conflict.provider_entity_id, "874");
        assert_eq!(
            conflict.entity_ids,
            vec![String::from("player-1"), String::from("player-2")],
            "both ids are reported, sorted, and neither is chosen"
        );
        assert_eq!(
            resolution
                .accepted
                .iter()
                .map(|link| link.entity_id.as_str())
                .collect::<Vec<_>>(),
            vec!["player-3"],
            "a conflicting identity does not block the rest of the batch"
        );
    }

    /// Given proposals keyed only by a provider id, from two providers
    /// When the batch resolves
    /// Then provider id alone is not the key: two providers keep two links.
    #[test]
    fn identity_is_keyed_by_provider_and_provider_id_not_by_id_alone() {
        let resolution = resolve_links(&[
            candidate("874", "player-1", 80),
            EntityLinkCandidate::new("local-fixture", "874", "player", "player-2", 80),
        ])
        .expect("two providers can disagree about one number");

        assert!(resolution.conflicts.is_empty());
        assert_eq!(resolution.accepted.len(), 2);
    }

    /// Given a proposal that also differs in entity kind
    /// When the batch resolves
    /// Then a club and a player may share a provider id without colliding.
    #[test]
    fn entity_kind_separates_two_entities_with_one_provider_id() {
        let resolution = resolve_links(&[
            EntityLinkCandidate::new("api-football", "50", "club", "club-1", 90),
            EntityLinkCandidate::new("api-football", "50", "player", "player-9", 90),
        ])
        .expect("kinds are part of the key");

        assert_eq!(resolution.accepted.len(), 2);
        assert!(resolution.conflicts.is_empty());
    }

    /// Given proposals with a blank provider, provider id, kind or world id
    /// When the batch resolves
    /// Then it is refused naming the blank field, so nothing is written.
    #[test]
    fn a_blank_key_field_is_refused() {
        let cases = [
            EntityLinkCandidate::new(" ", "874", "player", "player-1", 80),
            EntityLinkCandidate::new("api-football", "  ", "player", "player-1", 80),
            EntityLinkCandidate::new("api-football", "874", "", "player-1", 80),
            EntityLinkCandidate::new("api-football", "874", "player", "\t", 80),
        ];
        for case in cases {
            let error = resolve_links(std::slice::from_ref(&case))
                .expect_err("a blank key field cannot be written");
            assert!(
                matches!(error, WorldSyncError::BlankField { .. }),
                "{case:?} gave {error:?}"
            );
        }
    }

    /// Given a confidence above the contract's ceiling
    /// When the batch resolves
    /// Then it is refused rather than clamped silently.
    #[test]
    fn a_confidence_above_one_hundred_is_refused() {
        let error = resolve_links(&[candidate("874", "player-1", 101)])
            .expect_err("101 is outside 0..=100");
        assert_eq!(
            error,
            WorldSyncError::ConfidenceOutOfRange { confidence: 101 }
        );
    }

    /// Given the same proposals in a different order
    /// When both batches resolve
    /// Then the result is identical: paging, writing and reporting are order-independent.
    #[test]
    fn resolution_does_not_depend_on_proposal_order() {
        let forwards = vec![
            candidate("900", "player-9", 55).observed_on("2026-11-01"),
            candidate("900", "player-9", 55).observed_on("2026-11-15"),
            candidate("874", "player-1", 80),
            candidate("874", "player-2", 30),
        ];
        let backwards: Vec<EntityLinkCandidate> = forwards.iter().rev().cloned().collect();

        let first = resolve_links(&forwards).expect("valid");
        let second = resolve_links(&backwards).expect("valid");
        assert_eq!(first, second);
        assert_eq!(first.conflicts.len(), 1);
        assert_eq!(first.accepted.len(), 1);
        assert_eq!(first.accepted[0].provider_entity_id, "900");
        assert_eq!(first.accepted[0].as_of.as_deref(), Some("2026-11-15"));
    }

    /// Given an empty batch
    /// When it resolves
    /// Then nothing is accepted and nothing conflicts.
    #[test]
    fn an_empty_batch_resolves_to_nothing() {
        let resolution = resolve_links(&[]).expect("an empty batch is valid");
        assert_eq!(resolution, CanonicalResolution::default());
    }

    /// Given a proposal whose ids carry surrounding spaces
    /// When it resolves
    /// Then the link stores the trimmed ids, so two spellings cannot become two identities.
    #[test]
    fn surrounding_spaces_are_trimmed_out_of_a_link() {
        let resolution = resolve_links(&[EntityLinkCandidate::new(
            " api-football ",
            " 874 ",
            " player ",
            " player-1 ",
            80,
        )])
        .expect("valid");

        let link = &resolution.accepted[0];
        assert_eq!(link.provider, "api-football");
        assert_eq!(link.provider_entity_id, "874");
        assert_eq!(link.entity_kind, "player");
        assert_eq!(link.entity_id, "player-1");
    }
}
