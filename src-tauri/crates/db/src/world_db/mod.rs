//! The world database: a separate, read-mostly SQLite file holding an entire football world.
//!
//! A career save is one in-memory `Game` written whole on every save; at hundreds of thousands of
//! players that is neither fast nor possible. `world.db` is the layer that scales: indexed
//! identity rows, season statistics and membership periods, searched and paged by SQL, with a
//! career referring to them by id. It is deliberately a *separate file with its own migrations*
//! (`src/world_sql/w001_...`), so nothing here touches the career save schema or its persistence.
//!
//! The read contract — filters, sorts, page and profile shapes, name folding — lives in
//! `ofm_core::world_query`. This module supplies the SQLite implementation: opening and
//! migrating the file, bulk import from a generated world, and the indexed queries.
//!
//! Two paging styles are offered. An offset page counts the rows it skips; a keyset page seeks past
//! a `(sort value, player id)` position instead, so a deep page costs a seek rather than the whole
//! prefix. Both read count and rows inside one deferred read transaction, so a page and its total
//! are one snapshot even while [`SyncStore`] writes.
//!
//! M1 shipped no career wiring: the importer is fed a [`WorldSnapshot`], which
//! [`WorldSnapshot::from_world_data`] builds from the generator's output. The sync half — request
//! log, identity cross-walk, run ledger and competitions — is [`SyncStore`], and its policy lives
//! in `ofm_core::world_sync`.
//!
//! The career half is [`load_working_set`]: the clubs of the competitions a career plays in, their
//! players and a bounded set of free agents, materialised into `domain` types. The cost of that load
//! follows the working set, not the size of the file; anything outside it is reached on demand with
//! [`materialize_player`] or [`materialize_club_squad`].

mod career_bridge;
mod importer;
mod migrations;
mod queries;
mod snapshot;
mod sync_store;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use ofm_core::world_query::{
    Page, PlayerKeysetCursor, PlayerKeysetPage, PlayerPage, PlayerProfile, PlayerSearch,
    PositionFilter,
};

pub use career_bridge::{
    WorkingSetBundle, WorkingSetRequest, load_working_set, materialize_club_squad,
    materialize_player,
};
pub use importer::ImportSummary;
pub use migrations::{WORLD_MIGRATION_COUNT, world_migrations};
pub use snapshot::{
    ClubRecord, EntityLinkRecord, FetchLogRecord, MembershipRecord, PlayerRecord, SeasonStatRecord,
    WorldSnapshot,
};
pub use sync_store::{
    CompetitionClubRow, CompetitionRow, FetchLogEntry, LinkOutcome, NewFetchEntry, SyncRun,
    SyncStore,
};

/// The file name a world database is created under inside an app data directory.
pub const WORLD_DATABASE_FILE: &str = "world.db";

/// What can go wrong opening, importing into or querying a world database.
#[derive(Debug)]
pub enum WorldDbError {
    /// SQLite refused a statement, or the file could not be opened.
    Sqlite(rusqlite::Error),
    /// The world migration set could not be applied.
    Migration(String),
    /// A snapshot referred to a club or player it did not carry, or reused an id.
    InvalidSnapshot(String),
    /// A value from the generated world could not be stored (its attributes JSON).
    Encode(String),
    /// A query the caller built cannot be executed — a keyset cursor from another order, say.
    InvalidQuery(String),
    /// A sync row was malformed: a blank id, a timestamp that is not RFC 3339 UTC.
    InvalidSync(String),
}

impl fmt::Display for WorldDbError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "world database: {error}"),
            Self::Migration(error) => write!(formatter, "world database migration: {error}"),
            Self::InvalidSnapshot(error) => write!(formatter, "world snapshot: {error}"),
            Self::Encode(error) => write!(formatter, "world snapshot encoding: {error}"),
            Self::InvalidQuery(error) => write!(formatter, "world query: {error}"),
            Self::InvalidSync(error) => write!(formatter, "world sync: {error}"),
        }
    }
}

impl std::error::Error for WorldDbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Migration(_)
            | Self::InvalidSnapshot(_)
            | Self::Encode(_)
            | Self::InvalidQuery(_)
            | Self::InvalidSync(_) => None,
        }
    }
}

impl From<rusqlite::Error> for WorldDbError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

/// A world database opened at its latest schema version.
pub struct WorldDatabase {
    conn: Connection,
    path: Option<PathBuf>,
}

impl WorldDatabase {
    /// Open (or create) a world database file and apply its migrations.
    pub fn open(path: &Path) -> Result<Self, WorldDbError> {
        let mut conn = Connection::open(path)?;
        prepare(&conn)?;
        world_migrations()
            .to_latest(&mut conn)
            .map_err(|error| WorldDbError::Migration(error.to_string()))?;
        Ok(Self {
            conn,
            path: Some(path.to_path_buf()),
        })
    }

    /// An in-memory world database, for tests and for a world that has not been persisted yet.
    pub fn open_in_memory() -> Result<Self, WorldDbError> {
        let mut conn = Connection::open_in_memory()?;
        prepare(&conn)?;
        world_migrations()
            .to_latest(&mut conn)
            .map_err(|error| WorldDbError::Migration(error.to_string()))?;
        Ok(Self { conn, path: None })
    }

    /// The underlying connection, for `EXPLAIN QUERY PLAN` and for the sync layer's own staging
    /// statements. Read-only callers should prefer the typed queries below.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// The file this database lives in, or `None` for an in-memory one.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The applied world schema version (the number of `wNNN` migrations).
    pub fn schema_version(&self) -> Result<i64, WorldDbError> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// Replace the whole world with `snapshot`, in one transaction.
    ///
    /// A snapshot is a full world, not a delta: stale players and clubs are removed, so importing
    /// the same snapshot twice leaves exactly the same rows (no duplicates) and importing a
    /// corrected one cannot leave a removed player behind. The staging tables (`entity_links`,
    /// `fetch_log`) are cleared too; the sync layer writes them after a world import.
    pub fn import(&mut self, snapshot: &WorldSnapshot) -> Result<ImportSummary, WorldDbError> {
        importer::import(&mut self.conn, snapshot)
    }

    /// One page of players matching `search`, ordered by its sort and counted against the whole
    /// filtered set.
    pub fn search_players_page(
        &self,
        search: &PlayerSearch,
        page: Page,
    ) -> Result<PlayerPage, WorldDbError> {
        queries::search_players_page(&self.conn, search, page)
    }

    /// One page of a club's squad, optionally limited to a position or position group.
    pub fn club_squad_page(
        &self,
        club_id: &str,
        position: Option<PositionFilter>,
        page: Page,
    ) -> Result<PlayerPage, WorldDbError> {
        queries::club_squad_page(&self.conn, club_id, position, page)
    }

    /// One page of players found by seeking past `cursor` instead of counting an `OFFSET`.
    ///
    /// The first page passes `None`; every later page passes the previous page's `next_cursor`.
    /// `total` is counted in the same snapshot as the rows.
    pub fn search_players_keyset(
        &self,
        search: &PlayerSearch,
        limit: u32,
        cursor: Option<&PlayerKeysetCursor>,
    ) -> Result<PlayerKeysetPage, WorldDbError> {
        queries::search_players_keyset(&self.conn, search, limit, cursor)
    }

    /// The write side the live-sync layer owns: the request log, the identity cross-walk, the run
    /// ledger and competitions.
    pub fn sync(&self) -> SyncStore<'_> {
        SyncStore::new(&self.conn)
    }

    /// A player's profile: identity, stored attributes, membership history and season history.
    pub fn get_player_profile(
        &self,
        player_id: &str,
    ) -> Result<Option<PlayerProfile>, WorldDbError> {
        queries::get_player_profile(&self.conn, player_id)
    }
}

/// Connection settings every world database gets: foreign keys enforced (the schema relies on
/// cascades and on refusing an orphan), WAL so a reader never blocks the importer, and a busy
/// timeout so two processes opening the same world wait instead of failing.
fn prepare(conn: &Connection) -> Result<(), WorldDbError> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Returns the resulting mode. An in-memory database answers "memory"; a file answers "wal".
    // The mode is a property of the file, so this is idempotent on every open.
    let _mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}

#[cfg(test)]
mod career_bridge_tests;
#[cfg(test)]
mod tests;
