//! The write side of `world.db` that the live-sync layer owns.
//!
//! Four tables, one reason each:
//!
//! - `fetch_log` (w001) — what was asked for, so a sync can resume and skip an unchanged payload.
//! - `entity_links` (w001) — the only place two identities can be said to be the same entity.
//! - `world_sync_runs` (w002) — one row per attempt, holding the conditional-request state the next
//!   run reads (ETag, expiry) and the outcome a screen shows.
//! - `world_competitions` / `world_competition_clubs` (w002) — the competitions a world defines and
//!   who plays in them, by season.
//!
//! Timestamps are stored as RFC 3339 UTC at second precision. That is the only format this store
//! writes, so a range read or a prune compares text as time; rows another writer put in a different
//! format stay untouched rather than being pruned by a comparison that does not mean what it says.

use chrono::{DateTime, SecondsFormat, Utc};
use ofm_core::world_sync::{CanonicalLink, RouteFreshness};
use rusqlite::{Connection, OptionalExtension, params};

use crate::world_db::WorldDbError;

/// The confidence unit `entity_links` stores, as w001 declares it.
const LINK_CONFIDENCE_MAX: f64 = 1.0;

/// A request worth remembering, before the store assigns it a row id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFetchEntry {
    pub provider: String,
    /// The route or endpoint, as the adapter named it.
    pub endpoint: String,
    /// The HTTP status, or a provider-specific code; `None` when the attempt never got one.
    pub status: Option<i64>,
    pub fetched_at: DateTime<Utc>,
    pub payload_hash: Option<String>,
}

/// A stored request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchLogEntry {
    pub id: i64,
    pub provider: String,
    pub endpoint: String,
    pub status: Option<i64>,
    pub fetched_at: DateTime<Utc>,
    pub payload_hash: Option<String>,
}

/// What an upsert did to the identity cross-walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkOutcome {
    /// The identity was new.
    Inserted,
    /// The identity already pointed at this entity; its confidence or date moved.
    Updated,
    /// Another canonical id already claims this provider identity. Nothing was written: the two
    /// ids are a person's decision, not this store's.
    Conflict { existing_entity_id: String },
}

/// A provider identity `entity_links` holds two canonical ids for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityLinkConflict {
    pub provider: String,
    pub provider_id: String,
    pub entity_kind: String,
    pub entity_ids: Vec<String>,
}

/// One sync attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRun {
    pub run_id: String,
    pub provider: String,
    pub route: String,
    /// `dry-run` or `live`, from `world_sync::SyncMode`.
    pub mode: String,
    /// `planned`, `ok`, `not-modified`, `failed`, ...
    pub status: String,
    pub etag: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    /// `None` while a run is still in flight.
    pub finished_at: Option<DateTime<Utc>>,
    pub records_upserted: u64,
    pub note: Option<String>,
}

/// A competition as it is stored.
///
/// Separate from the adapter's `CompetitionRecord` on purpose: that is what a payload said, this is
/// the row, and the mapping between them — including a `tier` the provider never states — stays
/// with the caller rather than being buried in an insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompetitionRow {
    pub id: String,
    pub name: String,
    pub short_name: String,
    pub country: Option<String>,
    pub tier: i64,
    pub kind: String,
    pub confederation: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// One club's membership of one competition in one season.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompetitionClubRow {
    pub competition_id: String,
    pub club_id: String,
    /// The calendar year the season began in.
    pub season_year: i64,
}

/// The sync tables, seen through an open world database.
pub struct SyncStore<'conn> {
    conn: &'conn Connection,
}

impl<'conn> SyncStore<'conn> {
    /// A store over `conn`.
    pub fn new(conn: &'conn Connection) -> Self {
        Self { conn }
    }

    /// Remember one request, returning the row id it was given.
    pub fn record_fetch(&self, entry: &NewFetchEntry) -> Result<i64, WorldDbError> {
        require_text("the fetch log provider", &entry.provider)?;
        require_text("the fetch log endpoint", &entry.endpoint)?;
        self.conn.execute(
            "INSERT INTO fetch_log (provider, endpoint, status, fetched_at, payload_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.provider.trim(),
                entry.endpoint.trim(),
                entry.status,
                timestamp_text(entry.fetched_at),
                entry.payload_hash,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The newest request for one route, which is what a resume reads.
    pub fn latest_fetch(
        &self,
        provider: &str,
        endpoint: &str,
    ) -> Result<Option<FetchLogEntry>, WorldDbError> {
        let row = self
            .conn
            .query_row(
                "SELECT id, provider, endpoint, status, fetched_at, payload_hash
                 FROM fetch_log
                 WHERE provider = ?1 AND endpoint = ?2
                 ORDER BY fetched_at DESC, id DESC
                 LIMIT 1",
                params![provider.trim(), endpoint.trim()],
                fetch_entry_row,
            )
            .optional()?;
        row.transpose()
    }

    /// Every request one provider made, oldest first.
    pub fn fetch_entries(&self, provider: &str) -> Result<Vec<FetchLogEntry>, WorldDbError> {
        let mut statement = self.conn.prepare(
            "SELECT id, provider, endpoint, status, fetched_at, payload_hash
             FROM fetch_log
             WHERE provider = ?1
             ORDER BY fetched_at ASC, id ASC",
        )?;
        let rows = statement.query_map([provider.trim()], fetch_entry_row)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row??);
        }
        Ok(entries)
    }

    /// Delete every request fetched before `cutoff`, returning how many went.
    ///
    /// `datetime()` rather than a bare text comparison: a row another writer stored as `2026-11-19`
    /// is still prunable, and one in a format SQLite cannot read is left alone instead of being
    /// compared as nonsense. The cost is a scan, which a once-a-run prune carries comfortably.
    pub fn prune_fetch_log_before(&self, cutoff: DateTime<Utc>) -> Result<usize, WorldDbError> {
        let deleted = self.conn.execute(
            "DELETE FROM fetch_log WHERE datetime(fetched_at) < datetime(?1)",
            params![timestamp_text(cutoff)],
        )?;
        Ok(deleted)
    }

    /// Record (or confirm) a provider identity's canonical world entity.
    ///
    /// `link.confidence` is the sync contract's `0..=100`; the table declares `0.0..=1.0`, so the
    /// unit changes here and nowhere else.
    pub fn upsert_entity_link(
        &self,
        link: &CanonicalLink,
        reviewer: Option<&str>,
    ) -> Result<LinkOutcome, WorldDbError> {
        require_text("the link provider", &link.provider)?;
        require_text("the link provider entity id", &link.provider_entity_id)?;
        require_text("the link entity kind", &link.entity_kind)?;
        require_text("the link entity id", &link.entity_id)?;
        if link.confidence > 100 {
            return Err(WorldDbError::InvalidSync(format!(
                "a link confidence of {} is outside 0..=100",
                link.confidence
            )));
        }

        let existing = self.entity_ids_for(
            link.provider.trim(),
            link.provider_entity_id.trim(),
            link.entity_kind.trim(),
        )?;
        if let Some(other) = existing
            .iter()
            .find(|entity_id| entity_id.as_str() != link.entity_id.trim())
        {
            return Ok(LinkOutcome::Conflict {
                existing_entity_id: other.clone(),
            });
        }

        let confidence = f64::from(link.confidence) / 100.0 * LINK_CONFIDENCE_MAX;
        self.conn.execute(
            "INSERT INTO entity_links
                 (provider, provider_id, entity_kind, entity_id, confidence, reviewer, as_of)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (provider, provider_id, entity_kind, entity_id)
             DO UPDATE SET confidence = excluded.confidence, reviewer = excluded.reviewer,
                           as_of = excluded.as_of",
            params![
                link.provider.trim(),
                link.provider_entity_id.trim(),
                link.entity_kind.trim(),
                link.entity_id.trim(),
                confidence,
                reviewer,
                link.as_of.as_deref().map(str::trim).unwrap_or_default(),
            ],
        )?;
        Ok(match existing.is_empty() {
            true => LinkOutcome::Inserted,
            false => LinkOutcome::Updated,
        })
    }

    /// The canonical world id one provider identity resolves to, when it resolves to exactly one.
    ///
    /// `None` covers both "never linked" and "two ids disagree": both mean this identity is not
    /// usable without a person looking at it.
    pub fn resolve_entity_link(
        &self,
        provider: &str,
        provider_id: &str,
        entity_kind: &str,
    ) -> Result<Option<String>, WorldDbError> {
        let entity_ids =
            self.entity_ids_for(provider.trim(), provider_id.trim(), entity_kind.trim())?;
        Ok(match entity_ids.as_slice() {
            [only] => Some(only.clone()),
            _ => None,
        })
    }

    /// Every canonical id one provider identity holds, sorted.
    pub fn entity_ids_for(
        &self,
        provider: &str,
        provider_id: &str,
        entity_kind: &str,
    ) -> Result<Vec<String>, WorldDbError> {
        let mut statement = self.conn.prepare(
            "SELECT entity_id FROM entity_links
             WHERE provider = ?1 AND provider_id = ?2 AND entity_kind = ?3
             ORDER BY entity_id ASC",
        )?;
        let rows = statement.query_map(
            params![provider.trim(), provider_id.trim(), entity_kind.trim()],
            |row| row.get::<_, String>(0),
        )?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The stored link for one identity, or `None` when it is missing or two ids disagree.
    ///
    /// An ambiguous identity is not "the first one": the caller asked for the entity this provider
    /// row is, and two answers mean there is no answer without a person choosing.
    pub fn canonical_link(
        &self,
        provider: &str,
        provider_id: &str,
        entity_kind: &str,
    ) -> Result<Option<CanonicalLink>, WorldDbError> {
        let entity_ids = self.entity_ids_for(provider, provider_id, entity_kind)?;
        let [entity_id] = entity_ids.as_slice() else {
            return Ok(None);
        };
        let row = self
            .conn
            .query_row(
                "SELECT provider, provider_id, entity_kind, confidence, as_of
                 FROM entity_links
                 WHERE provider = ?1 AND provider_id = ?2 AND entity_kind = ?3 AND entity_id = ?4",
                params![
                    provider.trim(),
                    provider_id.trim(),
                    entity_kind.trim(),
                    entity_id
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((provider, provider_id, entity_kind, confidence, as_of)) = row else {
            return Ok(None);
        };
        Ok(Some(CanonicalLink {
            provider,
            provider_entity_id: provider_id,
            entity_kind,
            entity_id: entity_id.clone(),
            confidence: confidence_to_score(confidence),
            as_of: as_of.filter(|date| !date.is_empty()),
        }))
    }

    /// Every provider identity two canonical ids disagree about.
    pub fn conflicting_entity_links(&self) -> Result<Vec<EntityLinkConflict>, WorldDbError> {
        let mut statement = self.conn.prepare(
            "SELECT provider, provider_id, entity_kind, COUNT(DISTINCT entity_id) AS ids
             FROM entity_links
             GROUP BY provider, provider_id, entity_kind
             HAVING ids > 1
             ORDER BY provider, provider_id, entity_kind",
        )?;
        let keys = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut conflicts = Vec::with_capacity(keys.len());
        for (provider, provider_id, entity_kind) in keys {
            conflicts.push(EntityLinkConflict {
                entity_ids: self.entity_ids_for(&provider, &provider_id, &entity_kind)?,
                provider,
                provider_id,
                entity_kind,
            });
        }
        Ok(conflicts)
    }

    /// Record one sync attempt, replacing the row if the same run is recorded twice.
    pub fn record_sync_run(&self, run: &SyncRun) -> Result<(), WorldDbError> {
        require_text("the sync run id", &run.run_id)?;
        require_text("the sync run provider", &run.provider)?;
        require_text("the sync run route", &run.route)?;
        require_text("the sync run mode", &run.mode)?;
        require_text("the sync run status", &run.status)?;
        let records = i64::try_from(run.records_upserted).unwrap_or(i64::MAX);
        self.conn.execute(
            "INSERT INTO world_sync_runs
                 (run_id, provider, route, mode, status, etag, expires_at, started_at, finished_at,
                  records_upserted, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT (run_id) DO UPDATE SET
                 provider = excluded.provider, route = excluded.route, mode = excluded.mode,
                 status = excluded.status, etag = excluded.etag,
                 expires_at = excluded.expires_at, started_at = excluded.started_at,
                 finished_at = excluded.finished_at,
                 records_upserted = excluded.records_upserted, note = excluded.note",
            params![
                run.run_id.trim(),
                run.provider.trim(),
                run.route.trim(),
                run.mode.trim(),
                run.status.trim(),
                run.etag.as_deref(),
                run.expires_at.map(timestamp_text),
                timestamp_text(run.started_at),
                run.finished_at.map(timestamp_text),
                records,
                run.note.as_deref(),
            ],
        )?;
        Ok(())
    }

    /// One run by id.
    pub fn sync_run(&self, run_id: &str) -> Result<Option<SyncRun>, WorldDbError> {
        let row = self
            .conn
            .query_row(
                &format!("{RUN_SELECT} WHERE run_id = ?1"),
                [run_id.trim()],
                run_row,
            )
            .optional()?;
        row.transpose()
    }

    /// The newest run for one route.
    pub fn latest_sync_run(
        &self,
        provider: &str,
        route: &str,
    ) -> Result<Option<SyncRun>, WorldDbError> {
        let row = self
            .conn
            .query_row(
                &format!(
                    "{RUN_SELECT} WHERE provider = ?1 AND route = ?2 \
                     ORDER BY started_at DESC, run_id DESC LIMIT 1"
                ),
                params![provider.trim(), route.trim()],
                run_row,
            )
            .optional()?;
        row.transpose()
    }

    /// What the freshness policy needs for one route: the newest completed live run's validator
    /// and time, ignoring failed, in-flight, or dry runs.
    pub fn route_freshness(
        &self,
        provider: &str,
        route: &str,
    ) -> Result<Option<RouteFreshness>, WorldDbError> {
        let row = self
            .conn
            .query_row(
                &format!(
                    "{RUN_SELECT} WHERE provider = ?1 AND route = ?2 \
                     AND mode = 'live' AND status IN ('ok', 'not-modified') \
                     ORDER BY started_at DESC, run_id DESC LIMIT 1"
                ),
                params![provider.trim(), route.trim()],
                run_row,
            )
            .optional()?;
        Ok(row.transpose()?.map(|run| RouteFreshness {
            etag: run.etag,
            expires_at: run.expires_at,
            fetched_at: run.started_at,
        }))
    }

    /// Store or replace a competition.
    pub fn upsert_competition(&self, competition: &CompetitionRow) -> Result<(), WorldDbError> {
        require_text("the competition id", &competition.id)?;
        require_text("the competition name", &competition.name)?;
        require_text("the competition kind", &competition.kind)?;
        self.conn.execute(
            "INSERT INTO world_competitions
                 (id, name, short_name, country, tier, kind, confederation, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (id) DO UPDATE SET
                 name = excluded.name, short_name = excluded.short_name,
                 country = excluded.country, tier = excluded.tier, kind = excluded.kind,
                 confederation = excluded.confederation, updated_at = excluded.updated_at",
            params![
                competition.id.trim(),
                competition.name.trim(),
                competition.short_name.trim(),
                competition.country.as_deref().map(str::trim),
                competition.tier,
                competition.kind.trim(),
                competition.confederation.as_deref().map(str::trim),
                timestamp_text(competition.updated_at),
            ],
        )?;
        Ok(())
    }

    /// One competition by id.
    pub fn competition(&self, id: &str) -> Result<Option<CompetitionRow>, WorldDbError> {
        let row = self
            .conn
            .query_row(
                "SELECT id, name, short_name, country, tier, kind, confederation, updated_at
                 FROM world_competitions WHERE id = ?1",
                [id.trim()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, name, short_name, country, tier, kind, confederation, updated_at)) = row
        else {
            return Ok(None);
        };
        Ok(Some(CompetitionRow {
            id,
            name,
            short_name,
            country,
            tier,
            kind,
            confederation,
            updated_at: parse_timestamp(&updated_at)?,
        }))
    }

    /// Record that `club_id` plays in `competition_id` in `season_year`, idempotently.
    pub fn upsert_competition_club(
        &self,
        membership: &CompetitionClubRow,
    ) -> Result<(), WorldDbError> {
        require_text(
            "the competition club competition id",
            &membership.competition_id,
        )?;
        require_text("the competition club club id", &membership.club_id)?;
        self.conn.execute(
            "INSERT INTO world_competition_clubs (competition_id, club_id, season_year)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (competition_id, club_id, season_year) DO NOTHING",
            params![
                membership.competition_id.trim(),
                membership.club_id.trim(),
                membership.season_year,
            ],
        )?;
        Ok(())
    }

    /// The competitions one club played in, in one season.
    pub fn competitions_for_club(
        &self,
        club_id: &str,
        season_year: i64,
    ) -> Result<Vec<String>, WorldDbError> {
        self.ids_of(
            "SELECT competition_id FROM world_competition_clubs
             WHERE club_id = ?1 AND season_year = ?2 ORDER BY competition_id ASC",
            club_id,
            season_year,
        )
    }

    /// The clubs one competition fielded, in one season.
    pub fn competition_clubs(
        &self,
        competition_id: &str,
        season_year: i64,
    ) -> Result<Vec<String>, WorldDbError> {
        self.ids_of(
            "SELECT club_id FROM world_competition_clubs
             WHERE competition_id = ?1 AND season_year = ?2 ORDER BY club_id ASC",
            competition_id,
            season_year,
        )
    }

    fn ids_of(&self, sql: &str, key: &str, season_year: i64) -> Result<Vec<String>, WorldDbError> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map(params![key.trim(), season_year], |row| {
            row.get::<_, String>(0)
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

const RUN_SELECT: &str = "SELECT run_id, provider, route, mode, status, etag, expires_at, \
     started_at, finished_at, records_upserted, note FROM world_sync_runs";

/// A fetch-log row, with its timestamp parsed.
///
/// The `Result` is nested because a row exists but may hold a timestamp this store cannot read; the
/// caller decides whether that is skippable or fatal, and both callers here treat it as fatal.
fn fetch_entry_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<FetchLogEntry, WorldDbError>> {
    let id = row.get::<_, i64>(0)?;
    let provider = row.get::<_, String>(1)?;
    let endpoint = row.get::<_, String>(2)?;
    let status = row.get::<_, Option<i64>>(3)?;
    let fetched_at = row.get::<_, String>(4)?;
    let payload_hash = row.get::<_, Option<String>>(5)?;
    Ok(
        parse_timestamp(&fetched_at).map(|fetched_at| FetchLogEntry {
            id,
            provider,
            endpoint,
            status,
            fetched_at,
            payload_hash,
        }),
    )
}

fn run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<SyncRun, WorldDbError>> {
    let run_id: String = row.get(0)?;
    let provider: String = row.get(1)?;
    let route: String = row.get(2)?;
    let mode: String = row.get(3)?;
    let status: String = row.get(4)?;
    let etag: Option<String> = row.get(5)?;
    let expires_at: Option<String> = row.get(6)?;
    let started_at: String = row.get(7)?;
    let finished_at: Option<String> = row.get(8)?;
    let records_upserted: i64 = row.get(9)?;
    let note: Option<String> = row.get(10)?;

    Ok((|| {
        Ok(SyncRun {
            run_id,
            provider,
            route,
            mode,
            status,
            etag,
            expires_at: expires_at.as_deref().map(parse_timestamp).transpose()?,
            started_at: parse_timestamp(&started_at)?,
            finished_at: finished_at.as_deref().map(parse_timestamp).transpose()?,
            records_upserted: u64::try_from(records_upserted).unwrap_or(0),
            note,
        })
    })())
}

/// How a sync stores an instant: RFC 3339 UTC at second precision, so text order is time order.
pub fn timestamp_text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Read a stored instant, accepting the date-only form another writer may have used.
fn parse_timestamp(text: &str) -> Result<DateTime<Utc>, WorldDbError> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text.trim()) {
        return Ok(parsed.with_timezone(&Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
        && let Some(midnight) = date.and_hms_opt(0, 0, 0)
    {
        return Ok(midnight.and_utc());
    }
    Err(WorldDbError::InvalidSync(format!(
        "`{text}` is not an RFC 3339 UTC instant"
    )))
}

/// The sync contract's `0..=100` from the table's `0.0..=1.0`.
fn confidence_to_score(confidence: Option<f64>) -> u8 {
    let Some(confidence) = confidence else {
        return 0;
    };
    (confidence.clamp(0.0, LINK_CONFIDENCE_MAX) * 100.0).round() as u8
}

fn require_text(field: &str, value: &str) -> Result<(), WorldDbError> {
    if value.trim().is_empty() {
        return Err(WorldDbError::InvalidSync(format!("{field} is blank")));
    }
    Ok(())
}
