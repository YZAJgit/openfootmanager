-- The world database (`world.db`), schema version 1.
--
-- This is not a career save and shares none of its migrations: a world file is built from sync
-- and package imports, read far more often than it is written, and its shape is versioned in its
-- own `user_version` over the `wNNN_*.sql` set in this directory. Career saves keep their
-- `v001..v067` set untouched.
--
-- Missing is never zero. Every statistic a source may not report is nullable, and a reader must
-- render NULL as "unknown", not as a season of noughts.

CREATE TABLE world_clubs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    short_name TEXT NOT NULL DEFAULT '',
    -- The football nation the club plays in (`ENG`, `SCO`, ...), falling back to its location.
    country TEXT NOT NULL DEFAULT '',
    city TEXT NOT NULL DEFAULT '',
    stadium_name TEXT NOT NULL DEFAULT '',
    stadium_capacity INTEGER,
    reputation INTEGER,
    founded_year INTEGER,
    -- Authored kit colours, kept as the JSON object the package or generator gave.
    colors_json TEXT,
    play_style TEXT,
    -- Case-folded, diacritic-free search form (`world_query::fold_name`).
    name_normalized TEXT NOT NULL DEFAULT ''
);

CREATE INDEX idx_world_clubs_name ON world_clubs (name_normalized);

CREATE TABLE world_players (
    id TEXT PRIMARY KEY,
    first_name TEXT NOT NULL DEFAULT '',
    last_name TEXT NOT NULL DEFAULT '',
    full_name TEXT NOT NULL,
    match_name TEXT NOT NULL DEFAULT '',
    -- Case-folded, diacritic-free form (`world_query::fold_name`). FTS5 is a requirement, not an
    -- option: `world_player_search` fails to migrate without it, so this column is the name's sort
    -- key and the input a manual LIKE fallback would read, never an automatic fallback.
    name_normalized TEXT NOT NULL,
    -- Exact position code (`Striker`) and its group (`forward`), both from
    -- `world_query::position_code` / `position_group_code`.
    position TEXT NOT NULL,
    position_group TEXT NOT NULL,
    -- `YYYY-MM-DD` when known; either may be absent, and neither is ever a stand-in date.
    birth_date TEXT,
    birth_year INTEGER,
    nationality TEXT NOT NULL DEFAULT '',
    football_nation TEXT NOT NULL DEFAULT '',
    -- NULL for a free agent or a player whose club is not part of this world.
    current_club_id TEXT REFERENCES world_clubs (id) ON DELETE SET NULL,
    -- Ratings are nullable for the same reason statistics are: a source that did not say must
    -- not read as a 0-rated player.
    overall INTEGER,
    potential INTEGER,
    footedness TEXT,
    weak_foot INTEGER,
    -- The full attribute block as JSON text, the same trade the career save makes: indexed
    -- columns are for browsing, the blob is for the profile screen.
    attributes_json TEXT NOT NULL DEFAULT '{}',
    retired INTEGER NOT NULL DEFAULT 0 CHECK (retired IN (0, 1))
);

-- Every index leads with the equality filter a browse screen uses and continues with the default
-- sort (`overall DESC, id`) so a page is an index range, not a sort of the whole table.
CREATE INDEX idx_world_players_club_overall
    ON world_players (current_club_id, overall DESC, id);
CREATE INDEX idx_world_players_position_group
    ON world_players (position_group, overall DESC, id);
CREATE INDEX idx_world_players_position
    ON world_players (position, overall DESC, id);
CREATE INDEX idx_world_players_nationality
    ON world_players (nationality, overall DESC, id);
CREATE INDEX idx_world_players_overall ON world_players (overall DESC, id);
CREATE INDEX idx_world_players_potential ON world_players (potential DESC, id);
CREATE INDEX idx_world_players_name ON world_players (name_normalized, id);

-- Both directions of every sortable column, because an unfiltered browse must be an index walk
-- whichever way it is ordered: SQLite cannot walk `(overall DESC, id)` backwards into
-- `overall ASC, id ASC`, and before the birth-year indexes an age-ordered browse sorted the whole
-- table for every page.
CREATE INDEX idx_world_players_overall_ascending ON world_players (overall, id);
CREATE INDEX idx_world_players_potential_ascending ON world_players (potential, id);
CREATE INDEX idx_world_players_name_descending ON world_players (name_normalized DESC, id);
-- A NULL birth year leads an ascending browse and trails a descending one (SQLite orders NULLs
-- first); a NULL is a year the source never said, not the year zero.
CREATE INDEX idx_world_players_birth_year ON world_players (birth_year, id);
CREATE INDEX idx_world_players_birth_year_descending ON world_players (birth_year DESC, id);

CREATE TABLE world_player_season_stats (
    player_id TEXT NOT NULL REFERENCES world_players (id) ON DELETE CASCADE,
    -- The calendar year the season began in.
    season INTEGER NOT NULL,
    -- '' is the player's whole season across competitions; otherwise a competition id. A sentinel
    -- rather than NULL because SQLite primary keys treat NULLs as distinct and a re-import would
    -- then duplicate the aggregate row.
    competition_id TEXT NOT NULL DEFAULT '',
    club_id TEXT REFERENCES world_clubs (id) ON DELETE SET NULL,
    -- All nullable: a source reporting goals and no minutes leaves minutes unknown.
    appearances INTEGER,
    minutes_played INTEGER,
    goals INTEGER,
    assists INTEGER,
    clean_sheets INTEGER,
    yellow_cards INTEGER,
    red_cards INTEGER,
    avg_rating REAL,
    PRIMARY KEY (player_id, season, competition_id)
) WITHOUT ROWID;

CREATE INDEX idx_world_stats_club_season
    ON world_player_season_stats (club_id, season);

CREATE TABLE world_membership_periods (
    id INTEGER PRIMARY KEY,
    player_id TEXT NOT NULL REFERENCES world_players (id) ON DELETE CASCADE,
    -- NULL when the club is known only by name (an authored career row for a club outside the
    -- world), in which case `club_name` is all there is.
    club_id TEXT REFERENCES world_clubs (id) ON DELETE SET NULL,
    club_name TEXT NOT NULL DEFAULT '',
    -- Half-open validity interval. NULL `valid_from` means the start is unknown; NULL `valid_to`
    -- means the period is still open (the current club).
    valid_from TEXT,
    valid_to TEXT,
    kind TEXT NOT NULL DEFAULT 'career'
);

CREATE INDEX idx_world_membership_player
    ON world_membership_periods (player_id, valid_from);
CREATE INDEX idx_world_membership_club
    ON world_membership_periods (club_id, valid_from);

-- The cross-walk between a provider's identity and a local entity. Nothing writes it in the
-- foundation slice; the live-sync layer will. Identity is never merged by name, so this table is
-- the only place two providers' rows can be said to be the same person or club.
--
-- `entity_id` is deliberately not resolved against `world_players`/`world_clubs` here: a snapshot
-- imports staging rows as given, and the sync layer owns both the resolution check and the rule
-- for which link is canonical when one provider id proposes more than one local entity.
CREATE TABLE entity_links (
    provider TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    -- `player`, `club`, ... — the local entity's kind, and its id in the matching table.
    entity_kind TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    -- 0.0..=1.0; NULL when nobody rated the link.
    confidence REAL,
    -- Who confirmed it: a person, or the name of the importer that proposed it.
    reviewer TEXT,
    -- When the link was observed, `YYYY-MM-DD`.
    as_of TEXT NOT NULL,
    PRIMARY KEY (provider, provider_id, entity_kind, entity_id)
) WITHOUT ROWID;

CREATE INDEX idx_entity_links_entity ON entity_links (entity_kind, entity_id);

-- One row per provider request, so a sync can resume and say what it already asked for. Also
-- empty in the foundation slice. `status` is the HTTP status or a provider-specific code;
-- `payload_hash` is the response digest used to skip an unchanged payload. The sync layer upserts
-- or prunes its own rows so repeated syncs do not grow the log forever.
CREATE TABLE fetch_log (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    status INTEGER,
    fetched_at TEXT NOT NULL,
    payload_hash TEXT
);

CREATE INDEX idx_fetch_log_provider_time ON fetch_log (provider, fetched_at);
-- The resume lookup the sync layer makes: the newest fetch of one provider's one endpoint.
CREATE INDEX idx_fetch_log_route ON fetch_log (provider, endpoint, fetched_at);

-- Name search over players and their current clubs. Standalone (not external-content) because
-- the club name is not a column of `world_players`; the importer rebuilds it with the players.
-- The columns hold `world_query::fold_name` output, so "sukru" matches "Şükrü" whether or not
-- SQLite's tokenizer folds that letter, and every query token is a plain ASCII word.
CREATE VIRTUAL TABLE world_player_search USING fts5 (
    player_id UNINDEXED,
    name,
    club_name,
    tokenize = 'unicode61 remove_diacritics 2'
);
