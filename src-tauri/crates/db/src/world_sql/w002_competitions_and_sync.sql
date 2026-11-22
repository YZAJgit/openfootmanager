-- The world database (`world.db`), schema version 2: competitions and the sync ledger.
--
-- Append-only against `w001_world_schema.sql`, which is never edited: an existing world file
-- reports `user_version = 1`, applies this script once and reports 2. Nothing here drops or
-- rewrites a w001 table, so an M1 file keeps every player, club and statistic it already had.
--
-- Missing is never zero, as in w001: a competition whose source gave no confederation stores NULL,
-- and a reader renders that as unknown rather than inventing a value.

-- The competitions a world defines: leagues and cups, one row each. A competition is identified by
-- the world's own id, not a provider's, so the identity cross-walk stays in `entity_links`.
CREATE TABLE world_competitions (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    short_name TEXT NOT NULL,
    -- NULL when the source did not say which country the competition belongs to.
    country TEXT,
    -- 1 is a top division, 2 the second, and so on. A cup has no tier of its own and keeps 1.
    tier INTEGER NOT NULL DEFAULT 1,
    -- The caller's own code for the shape of the competition (`league`, `cup`, ...); the world
    -- schema does not enumerate them, because a package may introduce one the game does not know.
    kind TEXT NOT NULL,
    -- `UEFA`, `CONMEBOL`, ... or NULL for a domestic competition nobody attributed.
    confederation TEXT,
    -- When this row was last written, RFC 3339 UTC. A sync uses it to report staleness.
    updated_at TEXT NOT NULL
);

-- Which clubs take part in which competition, in which season.
--
-- No foreign keys on purpose. Live sync may record a competition's membership from a fixtures feed
-- before the clubs themselves are imported, and a fixture feed names clubs the world will only
-- later create; refusing the row would either lose the membership or force an import order the
-- provider does not guarantee. The sync layer owns resolution — see `world_sync::canonical` — and
-- the importer replaces the world atomically, so a stale row can never outlive its world.
CREATE TABLE world_competition_clubs (
    competition_id TEXT NOT NULL,
    club_id TEXT NOT NULL,
    -- The calendar year the season began in, matching `world_player_season_stats.season`.
    season_year INTEGER NOT NULL,
    PRIMARY KEY (competition_id, club_id, season_year)
) WITHOUT ROWID;

-- The reverse lookup the club screen makes: every competition a club played in.
CREATE INDEX idx_world_competition_clubs_club
    ON world_competition_clubs (club_id, season_year);

-- One sync attempt against one provider route. This is the run's own ledger, distinct from
-- `fetch_log` (w001), which records individual requests: a run holds the conditional-request state
-- (`etag`, `expires_at`) the next run reads to skip a fresh route, and the outcome a screen shows.
CREATE TABLE world_sync_runs (
    run_id TEXT PRIMARY KEY,
    -- `api-football`, ... — the same codes as `entity_links.provider`.
    provider TEXT NOT NULL,
    -- The route within the provider (`fixtures`, `players`), not a full URL: the adapter builds
    -- the URL and the key never enters the ledger.
    route TEXT NOT NULL,
    -- `dry-run` or `live`, from `world_sync::SyncMode`.
    mode TEXT NOT NULL,
    -- `planned`, `ok`, `not-modified`, `failed`, ...
    status TEXT NOT NULL,
    -- The provider's ETag for this route, echoed back as `If-None-Match` next time.
    etag TEXT,
    -- When the cached answer stops being usable, RFC 3339 UTC. NULL means the provider said nothing
    -- about freshness, so the next run revalidates conditionally rather than trusting the payload.
    expires_at TEXT,
    started_at TEXT NOT NULL,
    -- NULL while a run is still in flight.
    finished_at TEXT,
    records_upserted INTEGER NOT NULL DEFAULT 0,
    note TEXT
);

CREATE INDEX idx_world_sync_runs_route
    ON world_sync_runs (provider, route, started_at DESC);
