//! The SQL implementation of the world read contract (`ofm_core::world_query`).
//!
//! Every browse query is built from the filters the caller actually set, so SQLite can pick the
//! index that fits: club, group, exact position, nationality and rating range each lead an index,
//! and each continues with the default `overall DESC, id` order so a page is an index range
//! rather than a sort of the table. A name becomes an FTS5 `MATCH` against `world_player_search`,
//! which holds folded player and current-club names.
//!
//! Every sortable column is an index in both directions, so an unfiltered browse is always an
//! index walk. What stays bounded by the narrowed set — a name search, or a filter page sorted by
//! a column its index does not carry — is a sort of that set only, never of the table.
//!
//! Two paging styles share those statements. `OFFSET` paging counts the rows it skips, so page ten
//! thousand is ten thousand rows of work; keyset paging asks for the rows after a position instead,
//! and the branches below are written so SQLite turns each into an index *seek*. Both styles are
//! executed inside one deferred read transaction (`ReadSnapshot`), so a page and its total are one
//! snapshot even while the sync layer writes.

use ofm_core::world_query::{
    MembershipPeriod, Page, PlayerKeysetCursor, PlayerKeysetPage, PlayerPage, PlayerProfile,
    PlayerSearch, PlayerSort, PlayerSortColumn, PlayerSortValue, PlayerSummary, PositionFilter,
    SeasonStatLine, clamp_page_size,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params_from_iter};

use crate::world_db::WorldDbError;

/// The columns every player row needs, in the order [`player_summary`] reads them.
const SUMMARY_COLUMNS: &str = "p.id, p.full_name, p.position, p.position_group, p.nationality, \
     p.birth_date, p.birth_year, p.current_club_id, c.name, p.overall, p.potential";

/// One prepared statement (or its count twin) and the parameters the filters bind.
struct PlayerQuery {
    select: String,
    count: String,
    params: Vec<Value>,
}

/// The `FROM` and `WHERE` a search becomes, before a statement decides what to select.
struct SearchParts {
    from: String,
    conditions: Vec<String>,
    params: Vec<Value>,
}

impl SearchParts {
    fn where_clause(&self) -> String {
        match self.conditions.is_empty() {
            true => String::new(),
            false => format!(" WHERE {}", self.conditions.join(" AND ")),
        }
    }

    /// The filter's `WHERE`, narrowed further by `condition` from the seek.
    ///
    /// Parenthesised because the seek is a disjunction: without it `a = ? AND b > ? OR c` would
    /// bind tighter than the filter it must not escape.
    fn where_clause_with(&self, condition: &str) -> String {
        match self.conditions.is_empty() {
            true => format!(" WHERE {condition}"),
            false => format!(" WHERE {} AND ({condition})", self.conditions.join(" AND ")),
        }
    }
}

/// The two statements of one offset page, reading one snapshot.
pub(super) fn search_players_page(
    conn: &Connection,
    search: &PlayerSearch,
    page: Page,
) -> Result<PlayerPage, WorldDbError> {
    let page = Page::new(page.offset, page.limit);
    let query = build_search(search);

    let snapshot = ReadSnapshot::begin(conn)?;
    let total = count(snapshot.conn(), &query)?;
    let players = fetch_page(snapshot.conn(), &query, page)?;
    Ok(PlayerPage {
        players,
        total,
        offset: page.offset,
        limit: page.limit,
    })
}

pub(super) fn club_squad_page(
    conn: &Connection,
    club_id: &str,
    position: Option<PositionFilter>,
    page: Page,
) -> Result<PlayerPage, WorldDbError> {
    let search = PlayerSearch {
        club_id: Some(club_id.to_string()),
        position,
        ..PlayerSearch::default()
    };
    search_players_page(conn, &search, page)
}

/// One keyset page: the rows after `cursor`, in `search.sort`'s order.
///
/// `cursor` is `None` for the first page, which is the only page that starts at the beginning of
/// the order and the only one that counts the filtered set. A cursor from another order is refused
/// rather than paged with, because reading a position in the wrong direction silently skips or
/// repeats rows.
pub(super) fn search_players_keyset(
    conn: &Connection,
    search: &PlayerSearch,
    limit: u32,
    cursor: Option<&PlayerKeysetCursor>,
) -> Result<PlayerKeysetPage, WorldDbError> {
    let limit = clamp_page_size(limit);
    if let Some(cursor) = cursor {
        require_matching_order(search.sort, cursor)?;
    }
    let query = build_search(search);

    let snapshot = ReadSnapshot::begin(conn)?;
    // Only the first page counts: a count per seek page would scan the filtered set every time,
    // which is the cost seeking exists to remove.
    let total = match cursor {
        None => Some(count(snapshot.conn(), &query)?),
        Some(_) => None,
    };
    let players = fetch_keyset(snapshot.conn(), search, cursor, limit)?;

    // A short page means the set ended. A full one reports a cursor even when nothing follows, so
    // a caller may see one empty page rather than stopping early and losing the tail.
    let next_cursor = if players.len() == limit as usize {
        players
            .last()
            .map(|player| PlayerKeysetCursor::after(search.sort, player))
    } else {
        None
    };
    Ok(PlayerKeysetPage {
        players,
        total,
        limit,
        next_cursor,
    })
}

pub(super) fn get_player_profile(
    conn: &Connection,
    player_id: &str,
) -> Result<Option<PlayerProfile>, WorldDbError> {
    let snapshot = ReadSnapshot::begin(conn)?;
    let sql = format!(
        "SELECT {SUMMARY_COLUMNS}, p.attributes_json, p.footedness, p.weak_foot \
         FROM world_players p LEFT JOIN world_clubs c ON c.id = p.current_club_id \
         WHERE p.id = ?1"
    );
    let row = snapshot
        .conn()
        .query_row(&sql, [player_id], |row| {
            Ok((
                player_summary(row)?,
                row.get::<_, String>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<u8>>(13)?,
            ))
        })
        .optional()?;

    let Some((player, attributes_json, footedness, weak_foot)) = row else {
        return Ok(None);
    };

    let memberships = load_memberships(snapshot.conn(), player_id)?;
    let season_stats = load_season_stats(snapshot.conn(), player_id)?;
    Ok(Some(PlayerProfile {
        player,
        attributes_json,
        footedness,
        weak_foot,
        memberships,
        season_stats,
    }))
}

/// A deferred read transaction: the snapshot both statements of one page read.
///
/// Count and page are two statements, and without a transaction a sync writing between them makes
/// the total describe a world the rows do not. `unchecked_transaction` is `BEGIN DEFERRED` on a
/// shared `&Connection` — which is what keeps this out of the `&mut` the callers do not have — and
/// dropping the guard rolls back, so a read never commits and never leaves a lock behind.
struct ReadSnapshot<'conn> {
    transaction: Transaction<'conn>,
}

impl<'conn> ReadSnapshot<'conn> {
    fn begin(conn: &'conn Connection) -> Result<Self, WorldDbError> {
        Ok(Self {
            transaction: conn.unchecked_transaction()?,
        })
    }

    fn conn(&self) -> &Connection {
        &self.transaction
    }
}

fn load_memberships(
    conn: &Connection,
    player_id: &str,
) -> Result<Vec<MembershipPeriod>, WorldDbError> {
    let mut statement = conn.prepare(
        "SELECT m.club_id, COALESCE(NULLIF(m.club_name, ''), c.name, ''), m.valid_from, \
                m.valid_to, m.kind \
         FROM world_membership_periods m LEFT JOIN world_clubs c ON c.id = m.club_id \
         WHERE m.player_id = ?1 \
         ORDER BY COALESCE(m.valid_from, '') DESC, m.id ASC",
    )?;
    let rows = statement.query_map([player_id], |row| {
        Ok(MembershipPeriod {
            club_id: row.get(0)?,
            club_name: row.get(1)?,
            valid_from: row.get(2)?,
            valid_to: row.get(3)?,
            kind: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn load_season_stats(
    conn: &Connection,
    player_id: &str,
) -> Result<Vec<SeasonStatLine>, WorldDbError> {
    let mut statement = conn.prepare(
        "SELECT season, competition_id, club_id, appearances, minutes_played, goals, assists, \
                clean_sheets, yellow_cards, red_cards, avg_rating \
         FROM world_player_season_stats \
         WHERE player_id = ?1 \
         ORDER BY season DESC, competition_id ASC",
    )?;
    let rows = statement.query_map([player_id], |row| {
        Ok(SeasonStatLine {
            season: row.get(0)?,
            competition_id: row.get(1)?,
            club_id: row.get(2)?,
            appearances: row.get(3)?,
            minutes_played: row.get(4)?,
            goals: row.get(5)?,
            assists: row.get(6)?,
            clean_sheets: row.get(7)?,
            yellow_cards: row.get(8)?,
            red_cards: row.get(9)?,
            avg_rating: row.get(10)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn search_parts(search: &PlayerSearch) -> SearchParts {
    let mut from = String::from("world_players p");
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();

    if let Some(query) = search.name_query() {
        from.push_str(" JOIN world_player_search ON world_player_search.player_id = p.id");
        conditions.push(String::from("world_player_search MATCH ?"));
        params.push(Value::Text(query));
    }
    from.push_str(" LEFT JOIN world_clubs c ON c.id = p.current_club_id");

    if let Some(club_id) = &search.club_id {
        conditions.push(String::from("p.current_club_id = ?"));
        params.push(Value::Text(club_id.clone()));
    }
    if let Some(position) = &search.position {
        match position.group_code() {
            Some(group) => {
                conditions.push(String::from("p.position_group = ?"));
                params.push(Value::Text(group.to_string()));
            }
            None => {
                conditions.push(String::from("p.position = ?"));
                params.push(Value::Text(position.code().to_string()));
            }
        }
    }
    if let Some(nationality) = &search.nationality {
        conditions.push(String::from("p.nationality = ?"));
        params.push(Value::Text(nationality.clone()));
    }
    if let Some(minimum) = search.overall_min {
        conditions.push(String::from("p.overall >= ?"));
        params.push(Value::Integer(i64::from(minimum)));
    }
    if let Some(maximum) = search.overall_max {
        conditions.push(String::from("p.overall <= ?"));
        params.push(Value::Integer(i64::from(maximum)));
    }

    SearchParts {
        from,
        conditions,
        params,
    }
}

fn build_search(search: &PlayerSearch) -> PlayerQuery {
    let parts = search_parts(search);
    let where_clause = parts.where_clause();
    PlayerQuery {
        select: format!(
            "SELECT {SUMMARY_COLUMNS} FROM {} {where_clause} ORDER BY {} LIMIT ? OFFSET ?",
            parts.from,
            order_clause(search.sort)
        ),
        count: format!("SELECT COUNT(*) FROM {} {where_clause}", parts.from),
        params: parts.params,
    }
}

fn order_clause(sort: PlayerSort) -> String {
    let direction = if sort.descending { "DESC" } else { "ASC" };
    // Ties break on the primary key so two pages cannot show the same player twice or skip one.
    format!("{} {direction}, p.id ASC", column_name(sort.column))
}

fn column_name(column: PlayerSortColumn) -> &'static str {
    match column {
        PlayerSortColumn::Overall => "p.overall",
        PlayerSortColumn::Potential => "p.potential",
        PlayerSortColumn::Name => "p.name_normalized",
        PlayerSortColumn::BirthYear => "p.birth_year",
    }
}

fn count(conn: &Connection, query: &PlayerQuery) -> Result<u64, WorldDbError> {
    let total: i64 =
        conn.query_row(&query.count, params_from_iter(query.params.iter()), |row| {
            row.get(0)
        })?;
    Ok(u64::try_from(total).unwrap_or(0))
}

fn fetch_page(
    conn: &Connection,
    query: &PlayerQuery,
    page: Page,
) -> Result<Vec<PlayerSummary>, WorldDbError> {
    let mut params = query.params.clone();
    params.push(Value::Integer(i64::from(page.limit)));
    params.push(Value::Integer(
        i64::try_from(page.offset).unwrap_or(i64::MAX),
    ));
    fetch_rows(conn, &query.select, &params)
}

fn fetch_rows(
    conn: &Connection,
    sql: &str,
    params: &[Value],
) -> Result<Vec<PlayerSummary>, WorldDbError> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params_from_iter(params.iter()), player_summary)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// One keyset page, reading one snapshot.
///
/// Each branch is a contiguous slice of the index order, and the branches are concatenated in that
/// order, each with the same `ORDER BY` and a `LIMIT` for what the page still needs. Every branch is
/// a seek: the tie group first (`col = ? AND id > ?`), then the next values (`col < ?` or `col > ?`),
/// then the NULL group where the order puts it. Written as one `OR` instead, SQLite walks the whole
/// index for the same page as soon as the columns it must read are not all in the index.
fn fetch_keyset(
    conn: &Connection,
    search: &PlayerSearch,
    cursor: Option<&PlayerKeysetCursor>,
    limit: u32,
) -> Result<Vec<PlayerSummary>, WorldDbError> {
    let parts = search_parts(search);
    let mut players: Vec<PlayerSummary> = Vec::with_capacity(limit as usize);

    let Some(cursor) = cursor else {
        let mut params = parts.params.clone();
        params.push(Value::Integer(i64::from(limit)));
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM {} {} ORDER BY {} LIMIT ?",
            parts.from,
            parts.where_clause(),
            order_clause(search.sort)
        );
        players.extend(fetch_rows(conn, &sql, &params)?);
        return Ok(players);
    };

    for (condition, branch_params) in keyset_branches(search.sort, cursor) {
        let remaining = limit.saturating_sub(players.len() as u32);
        if remaining == 0 {
            break;
        }
        let mut params = parts.params.clone();
        params.extend(branch_params);
        params.push(Value::Integer(i64::from(remaining)));
        let sql = format!(
            "SELECT {SUMMARY_COLUMNS} FROM {} {} ORDER BY {} LIMIT ?",
            parts.from,
            parts.where_clause_with(&condition),
            order_clause(search.sort)
        );
        players.extend(fetch_rows(conn, &sql, &params)?);
    }
    Ok(players)
}

/// The conditions that page after `cursor`, in the order their rows appear in the index.
///
/// A NULL sort value is a real position: SQLite orders NULLs first, so ascending paging reaches
/// them before anything else and descending paging reaches them last. Each case therefore names the
/// part of the order after the cursor and nothing more.
fn keyset_branches(sort: PlayerSort, cursor: &PlayerKeysetCursor) -> Vec<(String, Vec<Value>)> {
    let column = column_name(sort.column);
    let player_id = Value::Text(cursor.player_id.clone());
    match &cursor.sort_value {
        PlayerSortValue::Text(value) => text_branches(column, sort.descending, value, player_id),
        PlayerSortValue::Number(value) => number_branches(column, sort, *value, player_id),
    }
}

fn number_branches(
    column: &str,
    sort: PlayerSort,
    value: Option<i32>,
    player_id: Value,
) -> Vec<(String, Vec<Value>)> {
    match (value, sort.descending) {
        // The cursor is inside the NULL group, which is the tail of a descending order.
        (None, true) => vec![(format!("{column} IS NULL AND p.id > ?"), vec![player_id])],
        // Ascending, inside the NULL group: the rest of it, then everything that is not NULL.
        (None, false) => vec![
            (format!("{column} IS NULL AND p.id > ?"), vec![player_id]),
            (format!("{column} IS NOT NULL"), Vec::new()),
        ],
        (Some(value), true) => vec![
            // The rest of this value's tie group comes first in the index.
            (
                format!("{column} = ? AND p.id > ?"),
                vec![Value::Integer(i64::from(value)), player_id],
            ),
            (
                format!("{column} < ?"),
                vec![Value::Integer(i64::from(value))],
            ),
            // NULLs trail a descending order, so they follow every non-null row after the cursor.
            (format!("{column} IS NULL"), Vec::new()),
        ],
        (Some(value), false) => vec![
            (
                format!("{column} = ? AND p.id > ?"),
                vec![Value::Integer(i64::from(value)), player_id],
            ),
            (
                format!("{column} > ?"),
                vec![Value::Integer(i64::from(value))],
            ),
        ],
    }
}

fn text_branches(
    column: &str,
    descending: bool,
    value: &str,
    player_id: Value,
) -> Vec<(String, Vec<Value>)> {
    let comparison = if descending { "<" } else { ">" };
    vec![
        (
            format!("{column} = ? AND p.id > ?"),
            vec![Value::Text(value.to_string()), player_id],
        ),
        (
            format!("{column} {comparison} ?"),
            vec![Value::Text(value.to_string())],
        ),
    ]
}

fn require_matching_order(
    sort: PlayerSort,
    cursor: &PlayerKeysetCursor,
) -> Result<(), WorldDbError> {
    if cursor.sort != sort {
        return Err(WorldDbError::InvalidQuery(format!(
            "a cursor taken in {cursor:?} cannot page an order of {sort:?}"
        )));
    }
    if cursor.player_id.trim().is_empty() {
        return Err(WorldDbError::InvalidQuery(String::from(
            "a keyset cursor without a player id is not a position",
        )));
    }
    let value_matches_column = matches!(
        (sort.column, &cursor.sort_value),
        (
            PlayerSortColumn::Overall | PlayerSortColumn::Potential | PlayerSortColumn::BirthYear,
            PlayerSortValue::Number(_)
        ) | (PlayerSortColumn::Name, PlayerSortValue::Text(_))
    );
    if !value_matches_column {
        return Err(WorldDbError::InvalidQuery(format!(
            "a cursor value {:?} cannot page a {:?} order",
            cursor.sort_value, sort.column
        )));
    }
    Ok(())
}

fn player_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlayerSummary> {
    Ok(PlayerSummary {
        id: row.get(0)?,
        full_name: row.get(1)?,
        position: row.get(2)?,
        position_group: row.get(3)?,
        nationality: row.get(4)?,
        birth_date: row.get(5)?,
        birth_year: row.get(6)?,
        current_club_id: row.get(7)?,
        current_club_name: row.get(8)?,
        overall: row.get(9)?,
        potential: row.get(10)?,
    })
}

#[cfg(test)]
pub(super) mod test_support {
    //! Hooks the world-db tests use to inspect the exact statement a query becomes, so
    //! `EXPLAIN QUERY PLAN` is run on the same SQL a caller runs, not on a hand-copied twin.

    use super::*;

    pub(in crate::world_db) fn search_sql(search: &PlayerSearch) -> (String, Vec<Value>) {
        let query = build_search(search);
        let mut params = query.params;
        // The select carries `LIMIT ? OFFSET ?`; EXPLAIN refuses a statement whose parameter
        // count does not match, so give the probe a real window.
        params.push(Value::Integer(10));
        params.push(Value::Integer(0));
        (query.select, params)
    }

    pub(in crate::world_db) fn count_sql(search: &PlayerSearch) -> (String, Vec<Value>) {
        let query = build_search(search);
        (query.count, query.params)
    }

    /// Every statement a keyset page would run, in the order it runs them.
    pub(in crate::world_db) fn keyset_sql(
        search: &PlayerSearch,
        cursor: Option<&PlayerKeysetCursor>,
        limit: u32,
    ) -> Vec<(String, Vec<Value>)> {
        let parts = search_parts(search);
        let Some(cursor) = cursor else {
            let mut params = parts.params.clone();
            params.push(Value::Integer(i64::from(limit)));
            return vec![(
                format!(
                    "SELECT {SUMMARY_COLUMNS} FROM {} {} ORDER BY {} LIMIT ?",
                    parts.from,
                    parts.where_clause(),
                    order_clause(search.sort)
                ),
                params,
            )];
        };
        keyset_branches(search.sort, cursor)
            .into_iter()
            .map(|(condition, branch_params)| {
                let mut params = parts.params.clone();
                params.extend(branch_params);
                params.push(Value::Integer(i64::from(limit)));
                (
                    format!(
                        "SELECT {SUMMARY_COLUMNS} FROM {} {} ORDER BY {} LIMIT ?",
                        parts.from,
                        parts.where_clause_with(&condition),
                        order_clause(search.sort)
                    ),
                    params,
                )
            })
            .collect()
    }

    /// The read-snapshot guard, so a test can hold one open while another connection writes.
    pub(in crate::world_db) struct OpenSnapshot<'conn>(ReadSnapshot<'conn>);

    impl<'conn> OpenSnapshot<'conn> {
        pub(in crate::world_db) fn begin(conn: &'conn Connection) -> Result<Self, WorldDbError> {
            Ok(Self(ReadSnapshot::begin(conn)?))
        }

        pub(in crate::world_db) fn conn(&self) -> &Connection {
            self.0.conn()
        }
    }
}
