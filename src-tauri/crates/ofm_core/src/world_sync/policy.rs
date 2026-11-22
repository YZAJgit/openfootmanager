//! Whether a route is worth asking for, and when a request may be forgotten.
//!
//! A sync that re-fetches everything on every run spends the user's quota on answers it already
//! has. The rule here is one sentence: a route with a cached answer inside its freshness window is
//! skipped, one outside it is asked again conditionally when the provider gave a validator, and
//! one with nothing remembered is fetched whole. Deciding that is pure arithmetic, so it is tested
//! without a clock, a file or a socket.

use chrono::{DateTime, Duration, Utc};

/// How long a request log entry is worth keeping.
///
/// A month covers "what did the sync do last time" and every resume window a provider offers; an
/// older row can only answer a question nobody asks again.
pub fn default_retention() -> Duration {
    Duration::days(30)
}

/// What a run should do for one route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncRequestPlan {
    /// The cached answer is still inside its freshness window; do not send anything.
    SkipFresh,
    /// Ask again, but let the provider answer "not modified" if it has nothing new.
    FetchConditional { if_none_match: String },
    /// Ask for the whole payload.
    FetchFull,
}

/// What the sync remembers about one route's last successful answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFreshness {
    /// The provider's validator, echoed back as `If-None-Match`.
    pub etag: Option<String>,
    /// When the cached answer stops being usable. `None` means the provider said nothing about
    /// freshness, so the next run revalidates rather than trusting an old payload.
    pub expires_at: Option<DateTime<Utc>>,
    /// When the cached answer was stored.
    pub fetched_at: DateTime<Utc>,
}

/// Decide what to do for `route` at `now`.
///
/// Freshness wins over a validator: an answer still inside its window is skipped even when the
/// provider also handed out an ETag, because a conditional request is still a request.
pub fn plan_request(now: DateTime<Utc>, freshness: Option<&RouteFreshness>) -> SyncRequestPlan {
    let Some(freshness) = freshness else {
        return SyncRequestPlan::FetchFull;
    };
    if freshness
        .expires_at
        .is_some_and(|expires_at| expires_at > now)
    {
        return SyncRequestPlan::SkipFresh;
    }
    match freshness.etag.as_deref().map(str::trim) {
        Some(etag) if !etag.is_empty() => SyncRequestPlan::FetchConditional {
            if_none_match: etag.to_string(),
        },
        _ => SyncRequestPlan::FetchFull,
    }
}

/// The instant a response fetched at `fetched_at` stops being fresh.
///
/// A negative or absurd TTL can come from a config file, so it is clamped and the addition is
/// checked: the result is at worst `fetched_at`, never a panic.
pub fn expires_at_from(fetched_at: DateTime<Utc>, ttl_seconds: i64) -> DateTime<Utc> {
    let ttl = Duration::try_seconds(ttl_seconds.max(0)).unwrap_or_else(Duration::zero);
    fetched_at.checked_add_signed(ttl).unwrap_or(fetched_at)
}

/// Everything fetched strictly before this instant may be pruned.
///
/// Strictness matters: an entry fetched exactly at the boundary is the oldest one a resume would
/// still consult.
pub fn prune_cutoff(now: DateTime<Utc>, retention: Duration) -> DateTime<Utc> {
    now.checked_sub_signed(retention.max(Duration::zero()))
        .unwrap_or(now)
}

/// Whether a request fetched at `fetched_at` is older than the pruning window.
pub fn should_prune(fetched_at: DateTime<Utc>, cutoff: DateTime<Utc>) -> bool {
    fetched_at < cutoff
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed instant, so no test depends on the wall clock.
    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("a representable instant")
    }

    fn freshness(etag: Option<&str>, expires_at: Option<i64>, fetched_at: i64) -> RouteFreshness {
        RouteFreshness {
            etag: etag.map(str::to_string),
            expires_at: expires_at.map(at),
            fetched_at: at(fetched_at),
        }
    }

    /// Given a route the sync has never asked for
    /// When a plan is made
    /// Then the whole payload is fetched.
    #[test]
    fn a_route_with_no_history_is_fetched_in_full() {
        assert_eq!(plan_request(at(1_000), None), SyncRequestPlan::FetchFull);
    }

    /// Given a cached answer still inside its freshness window and an ETag
    /// When a plan is made
    /// Then nothing is sent, because a conditional request is still a request.
    #[test]
    fn a_fresh_route_is_skipped_without_a_request() {
        let cached = freshness(Some("\"v1\""), Some(2_000), 1_000);
        assert_eq!(
            plan_request(at(1_500), Some(&cached)),
            SyncRequestPlan::SkipFresh
        );
    }

    /// Given an expired cached answer that came with an ETag
    /// When a plan is made
    /// Then it is revalidated conditionally with that ETag.
    #[test]
    fn an_expired_route_with_an_etag_is_revalidated_conditionally() {
        let cached = freshness(Some("\"v1\""), Some(2_000), 1_000);
        assert_eq!(
            plan_request(at(2_000), Some(&cached)),
            SyncRequestPlan::FetchConditional {
                if_none_match: String::from("\"v1\"")
            },
            "expiry is strict: an answer expiring now is no longer fresh"
        );
        assert_eq!(
            plan_request(at(9_000), Some(&cached)),
            SyncRequestPlan::FetchConditional {
                if_none_match: String::from("\"v1\"")
            }
        );
    }

    /// Given an expired answer with no usable validator
    /// When a plan is made
    /// Then the whole payload is fetched; there is nothing to revalidate against.
    #[test]
    fn an_expired_route_without_an_etag_is_fetched_in_full() {
        for etag in [None, Some(""), Some("   ")] {
            let cached = freshness(etag, Some(2_000), 1_000);
            assert_eq!(
                plan_request(at(5_000), Some(&cached)),
                SyncRequestPlan::FetchFull
            );
        }
        let never_expiring_unknown = freshness(None, None, 1_000);
        assert_eq!(
            plan_request(at(5_000), Some(&never_expiring_unknown)),
            SyncRequestPlan::FetchFull
        );
    }

    /// Given a TTL a settings file could plausibly carry, including a negative and an absurd one
    /// When the expiry is computed
    /// Then it never panics and never produces a tree-shaking value.
    #[test]
    fn expiry_math_survives_a_hostile_ttl() {
        let fetched = at(1_000);
        assert_eq!(expires_at_from(fetched, 60), at(1_060));
        assert_eq!(expires_at_from(fetched, -60), fetched);
        assert_eq!(expires_at_from(fetched, 0), fetched);
        assert!(expires_at_from(fetched, i64::MAX) >= fetched);
        assert_eq!(default_retention(), Duration::days(30));
    }

    /// Given a request log with entries either side of the retention window
    /// When the cutoff is computed and the entries are tested
    /// Then only the older ones are prunable, and the boundary entry is kept.
    #[test]
    fn only_entries_older_than_the_retention_window_are_pruned() {
        let now = at(10_000_000);
        let cutoff = prune_cutoff(now, default_retention());
        assert_eq!(cutoff, now - Duration::days(30));

        assert!(
            should_prune(at(1_000), cutoff),
            "a year-old entry is pruned"
        );
        assert!(
            !should_prune(cutoff, cutoff),
            "an entry exactly at the boundary is the oldest one a resume still consults"
        );
        assert!(!should_prune(at(9_999_999), cutoff));
        assert_eq!(
            prune_cutoff(now, Duration::seconds(-5)),
            now,
            "a negative retention prunes nothing"
        );
    }
}
