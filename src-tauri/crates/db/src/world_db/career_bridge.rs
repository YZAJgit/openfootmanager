//! The career <-> `world.db` bridge: a bounded working set, and lazy materialisation of anything
//! outside it.
//!
//! A career never holds the whole world. It loads the clubs of the competitions it plays in, the
//! players attached to those clubs, and a bounded handful of free agents — a few hundred rows out of
//! a world file that may hold a hundred thousand. Every read here is bounded by the clubs the
//! request names, so the cost of a load follows the working set rather than the size of the world.
//! A player the career has not loaded yet is materialised on demand from [`materialize_player`] or
//! [`materialize_club_squad`].
//!
//! Materialisation is a projection, one way only: `world_players` and `world_clubs` are read and
//! never written, so a statistic the source never reported stays NULL in the world file, and a value
//! derived to keep the in-memory career coherent (a rating from the attribute block, a market value
//! from the rating) lives only in the career. Missing is never zero.
//!
//! Everything is read inside one deferred read transaction, so the clubs, the players, the free
//! agents and the world total describe one snapshot even while the sync layer writes.

use std::collections::BTreeSet;

use chrono::NaiveDate;
use domain::player::{Footedness, Player, PlayerAttributes};
use domain::team::{PlayStyle, Team, TeamColors};
use ofm_core::player_rating::{estimate_market_value, ovr_from_attributes};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params_from_iter};

use crate::repositories::player_repo::parse_position;
use crate::world_db::{WorldDatabase, WorldDbError};

/// How many ids one `IN (...)` list carries. A club list is chunked rather than sent whole: the
/// number of bound variables is a per-statement limit, and a competition plus its cup ties can
/// easily exceed the 999 default on a SQLite build that has not raised it.
const ID_CHUNK: usize = 400;

/// What an attribute block that says nothing at all reads as: a neutral professional, neither
/// unrated (zero) nor elite.
const NEUTRAL_ATTRIBUTE: u8 = 50;

const CLUB_COLUMNS: &str = "id, name, short_name, country, city, stadium_name, stadium_capacity, \
     reputation, founded_year, colors_json, play_style";

const PLAYER_COLUMNS: &str = "id, full_name, match_name, position, birth_date, birth_year, \
     nationality, current_club_id, overall, potential, footedness, weak_foot, attributes_json, \
     retired";

/// What a career asks the world for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkingSetRequest {
    /// The competitions whose clubs the career simulates.
    pub competition_ids: Vec<String>,
    /// Clubs outside those competitions that the career also needs — a cup opponent, the club a
    /// transfer target plays for.
    pub extra_club_ids: Vec<String>,
    /// The season whose competition membership to read. `None` reads every season the world holds,
    /// which is what a caller with no season context yet wants.
    pub season_year: Option<i32>,
    /// How many free agents to materialise. `None` loads none: the transfer market pages its own.
    pub include_free_agents_limit: Option<usize>,
}

/// The working set, materialised into the in-memory career's own types.
#[derive(Debug, Clone)]
pub struct WorkingSetBundle {
    pub clubs: Vec<Team>,
    pub players: Vec<Player>,
    /// Every player the world holds, loaded or not, so a screen can say "412 of 100,000".
    pub total_world_players: u64,
    /// The clubs that were actually found, sorted. A requested id the world does not define is not
    /// an error — a stale competition membership is common — it is simply absent.
    pub loaded_club_ids: Vec<String>,
}

/// Load the clubs a request names, their players and a bounded set of free agents.
///
/// One deferred read transaction covers the whole load, including the world total, so the bundle is
/// internally consistent. Clubs and players come back ordered by id, which makes the conversion —
/// and a save written from it — reproducible from the same world file.
pub fn load_working_set(
    world: &WorldDatabase,
    request: &WorkingSetRequest,
    current_date: NaiveDate,
) -> Result<WorkingSetBundle, WorldDbError> {
    let transaction = world.conn().unchecked_transaction()?;
    let total_world_players = count_world_players(&transaction)?;
    let club_ids = requested_club_ids(&transaction, request)?;
    let clubs = load_clubs_in(&transaction, &club_ids)?;
    let mut players = load_players_in(&transaction, &club_ids, current_date)?;
    if let Some(limit) = request.include_free_agents_limit {
        players.extend(load_free_agents(&transaction, limit, current_date)?);
    }
    players.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(WorkingSetBundle {
        loaded_club_ids: clubs.iter().map(|club| club.id.clone()).collect(),
        clubs,
        players,
        total_world_players,
    })
}

/// Read one club's squad from the world, outside any working set.
///
/// A club the world does not define, or one with no players, is an empty squad rather than an
/// error: the caller asked "who plays here", and the honest answer is "nobody in this file".
pub fn materialize_club_squad(
    world: &WorldDatabase,
    club_id: &str,
    current_date: NaiveDate,
) -> Result<Vec<Player>, WorldDbError> {
    let club_id = club_id.trim();
    if club_id.is_empty() {
        return Ok(Vec::new());
    }
    let transaction = world.conn().unchecked_transaction()?;
    let club_ids = [club_id.to_string()];
    load_players_in(&transaction, &club_ids, current_date)
}

/// Read one player from the world, wherever he plays — a free agent, or a club the career has not
/// loaded. `None` when the world has no such id.
pub fn materialize_player(
    world: &WorldDatabase,
    player_id: &str,
    current_date: NaiveDate,
) -> Result<Option<Player>, WorldDbError> {
    let player_id = player_id.trim();
    if player_id.is_empty() {
        return Ok(None);
    }
    let transaction = world.conn().unchecked_transaction()?;
    let sql = format!("SELECT {PLAYER_COLUMNS} FROM world_players WHERE id = ?1");
    let stored = transaction
        .query_row(&sql, [player_id], stored_player)
        .optional()?;
    Ok(stored.map(|stored| player_from_stored(stored, current_date)))
}

/// The clubs a request names: the union of its extra ids and the requested competitions' members,
/// deduplicated and sorted. Non-blank ids only; a membership row may name a club the world has not
/// imported, which the club read then simply does not return.
fn requested_club_ids(
    conn: &Connection,
    request: &WorkingSetRequest,
) -> Result<Vec<String>, WorldDbError> {
    let mut ids: BTreeSet<String> = request
        .extra_club_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();

    let competitions: Vec<String> = request
        .competition_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();

    for chunk in competitions.chunks(ID_CHUNK) {
        let season_clause = match request.season_year {
            Some(_) => " AND season_year = ?",
            None => "",
        };
        let sql = format!(
            "SELECT DISTINCT club_id FROM world_competition_clubs \
             WHERE competition_id IN ({}){season_clause} ORDER BY club_id ASC",
            placeholders(chunk.len())
        );
        let mut params: Vec<Value> = chunk.iter().map(|id| Value::Text(id.clone())).collect();
        if let Some(season_year) = request.season_year {
            params.push(Value::Integer(i64::from(season_year)));
        }
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(params.iter()), |row| {
            row.get::<_, String>(0)
        })?;
        for row in rows {
            ids.insert(row?);
        }
    }
    Ok(ids.into_iter().collect())
}

/// How many player rows the world holds. Counted, never materialised: this is the number the
/// working set is small *against*, and reading it must not cost what reading them would.
fn count_world_players(conn: &Connection) -> Result<u64, WorldDbError> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM world_players", [], |row| row.get(0))?;
    Ok(u64::try_from(total).unwrap_or(0))
}

fn load_clubs_in(conn: &Connection, club_ids: &[String]) -> Result<Vec<Team>, WorldDbError> {
    let mut stored = Vec::with_capacity(club_ids.len());
    for chunk in club_ids.chunks(ID_CHUNK) {
        let sql = format!(
            "SELECT {CLUB_COLUMNS} FROM world_clubs WHERE id IN ({}) ORDER BY id ASC",
            placeholders(chunk.len())
        );
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(chunk.iter().map(String::as_str)),
            stored_club,
        )?;
        stored.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    let mut clubs: Vec<Team> = stored.into_iter().map(team_from_stored).collect();
    clubs.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(clubs)
}

fn load_players_in(
    conn: &Connection,
    club_ids: &[String],
    current_date: NaiveDate,
) -> Result<Vec<Player>, WorldDbError> {
    let mut stored = Vec::new();
    for chunk in club_ids.chunks(ID_CHUNK) {
        let sql = format!(
            "SELECT {PLAYER_COLUMNS} FROM world_players \
             WHERE current_club_id IN ({}) ORDER BY id ASC",
            placeholders(chunk.len())
        );
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(
            params_from_iter(chunk.iter().map(String::as_str)),
            stored_player,
        )?;
        stored.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    Ok(players_from_stored(stored, current_date))
}

/// Free agents in id order, so the same limit picks the same people from the same file every time.
fn load_free_agents(
    conn: &Connection,
    limit: usize,
    current_date: NaiveDate,
) -> Result<Vec<Player>, WorldDbError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT {PLAYER_COLUMNS} FROM world_players \
         WHERE current_club_id IS NULL ORDER BY id ASC LIMIT ?1"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], stored_player)?;
    let stored = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(players_from_stored(stored, current_date))
}

fn players_from_stored(stored: Vec<StoredPlayer>, current_date: NaiveDate) -> Vec<Player> {
    let mut players: Vec<Player> = stored
        .into_iter()
        .map(|player| player_from_stored(player, current_date))
        .collect();
    players.sort_by(|left, right| left.id.cmp(&right.id));
    players
}

/// `?` repeated `count` times, for an `IN (...)` list.
fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

/// A `world_clubs` row, read as stored. Separate from the `Team` it becomes so a row that cannot be
/// read fails as a SQLite error and a row whose optional columns are absent stays absent, rather
/// than the two being flattened into one default.
struct StoredClub {
    id: String,
    name: String,
    short_name: String,
    country: String,
    city: String,
    stadium_name: String,
    stadium_capacity: Option<i64>,
    reputation: Option<i64>,
    founded_year: Option<i64>,
    colors_json: Option<String>,
    play_style: Option<String>,
}

fn stored_club(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredClub> {
    Ok(StoredClub {
        id: row.get(0)?,
        name: row.get(1)?,
        short_name: row.get(2)?,
        country: row.get(3)?,
        city: row.get(4)?,
        stadium_name: row.get(5)?,
        stadium_capacity: row.get(6)?,
        reputation: row.get(7)?,
        founded_year: row.get(8)?,
        colors_json: row.get(9)?,
        play_style: row.get(10)?,
    })
}

/// A `world_players` row, read as stored. Everything the world may not have said is an `Option`:
/// a NULL rating, foot, birth date or weak foot stays `None` until materialisation decides what the
/// career shows, and is never written back.
struct StoredPlayer {
    id: String,
    full_name: String,
    match_name: String,
    position: String,
    birth_date: Option<String>,
    birth_year: Option<i64>,
    nationality: String,
    current_club_id: Option<String>,
    overall: Option<i64>,
    potential: Option<i64>,
    footedness: Option<String>,
    weak_foot: Option<i64>,
    attributes_json: String,
    retired: bool,
}

fn stored_player(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredPlayer> {
    Ok(StoredPlayer {
        id: row.get(0)?,
        full_name: row.get(1)?,
        match_name: row.get(2)?,
        position: row.get(3)?,
        birth_date: row.get(4)?,
        birth_year: row.get(5)?,
        nationality: row.get(6)?,
        current_club_id: row.get(7)?,
        overall: row.get(8)?,
        potential: row.get(9)?,
        footedness: row.get(10)?,
        weak_foot: row.get(11)?,
        attributes_json: row.get(12)?,
        retired: row.get(13)?,
    })
}

fn team_from_stored(stored: StoredClub) -> Team {
    let stadium_capacity = non_negative_u32(stored.stadium_capacity).unwrap_or(0);
    let mut team = Team::new(
        stored.id,
        stored.name,
        stored.short_name,
        stored.country,
        stored.city,
        stored.stadium_name,
        stadium_capacity,
    );
    if let Some(reputation) = non_negative_u32(stored.reputation) {
        team.reputation = reputation;
    }
    if let Some(founded_year) = non_negative_u32(stored.founded_year) {
        team.founded_year = founded_year;
    }
    if let Some(colors) = stored.colors_json.as_deref().and_then(colors_from_json) {
        team.colors = colors;
    }
    if let Some(play_style) = stored.play_style.as_deref().and_then(play_style_from_code) {
        team.play_style = play_style;
    }
    team
}

fn player_from_stored(stored: StoredPlayer, current_date: NaiveDate) -> Player {
    let position = parse_position(&stored.position);
    let stated_overall = rating(stored.overall);
    let attributes = attributes_from_json(&stored.attributes_json, stated_overall);
    let derived_overall = ovr_from_attributes(&attributes, &position).round() as u8;
    let date_of_birth = birth_date_text(&stored);
    let mut player = Player::new(
        stored.id,
        stored.match_name,
        stored.full_name,
        date_of_birth,
        stored.nationality,
        position,
        attributes,
    );

    // A rating the source did not state is derived from his attributes, never left as zero: zero is
    // a real "rated nothing", and no world row means that.
    player.ovr = stated_overall.unwrap_or(derived_overall);
    player.potential = rating(stored.potential)
        .unwrap_or(player.ovr)
        .max(player.ovr);
    player.team_id = non_empty(stored.current_club_id);
    if let Some(footedness) = stored.footedness.as_deref().and_then(footedness_from_code) {
        player.footedness = footedness;
    }
    if let Some(weak_foot) = stored.weak_foot.and_then(positive_u8) {
        player.weak_foot = weak_foot;
    }
    player.retired = stored.retired;
    let age = domain::age::playing_age(current_date, &player.date_of_birth);
    player.market_value = estimate_market_value(player.ovr, age);
    player
}

/// The birth date the career shows: the stored date when the world has one, otherwise the season's
/// midpoint of the year it did state — the game's own rule for a date known only to its year
/// (`domain::age::season_midpoint`), so a player with a year and no day still has a real age.
fn birth_date_text(stored: &StoredPlayer) -> String {
    if let Some(date) = stored
        .birth_date
        .as_deref()
        .map(str::trim)
        .filter(|date| !date.is_empty())
    {
        return date.to_string();
    }
    match stored.birth_year {
        Some(year) => {
            domain::age::season_midpoint(i32::try_from(year).unwrap_or_default()).to_string()
        }
        None => String::new(),
    }
}

/// The attribute block as the world stored it, with every field it did not state filled from the
/// overall it did state. An incomplete block is common — a package may author only the attributes a
/// position uses — and must degrade to a usable player rather than a failed conversion.
fn attributes_from_json(text: &str, stated_overall: Option<u8>) -> PlayerAttributes {
    let parsed: Option<serde_json::Value> = match serde_json::from_str(text) {
        Ok(value) => Some(value),
        Err(err) => {
            log::warn!("world_players.attributes_json parse failed: {err}");
            None
        }
    };
    let block = parsed.as_ref().and_then(serde_json::Value::as_object);
    let fallback = stated_overall.unwrap_or(NEUTRAL_ATTRIBUTE);

    let read = |name: &str| attribute_value(block, name).unwrap_or(fallback);
    PlayerAttributes {
        pace: read("pace"),
        stamina: read("stamina"),
        strength: read("strength"),
        agility: read("agility"),
        passing: read("passing"),
        shooting: read("shooting"),
        tackling: read("tackling"),
        dribbling: read("dribbling"),
        defending: read("defending"),
        positioning: read("positioning"),
        vision: read("vision"),
        decisions: read("decisions"),
        composure: read("composure"),
        aggression: read("aggression"),
        teamwork: read("teamwork"),
        leadership: read("leadership"),
        handling: read("handling"),
        reflexes: read("reflexes"),
        aerial: read("aerial"),
    }
}

/// One attribute from the stored block. `0` and anything outside `1..=99` read as "not stated",
/// because a source that never rated a field does not mean it rated it at nought.
fn attribute_value(
    block: Option<&serde_json::Map<String, serde_json::Value>>,
    name: &str,
) -> Option<u8> {
    let value = block?.get(name)?.as_u64()?;
    if !(1..=99).contains(&value) {
        return None;
    }
    u8::try_from(value).ok()
}

fn colors_from_json(text: &str) -> Option<TeamColors> {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            log::warn!("world_clubs.colors_json parse failed: {err}");
            return None;
        }
    };
    let primary = value.get("primary")?.as_str()?.trim();
    let secondary = value.get("secondary")?.as_str()?.trim();
    if primary.is_empty() || secondary.is_empty() {
        return None;
    }
    Some(TeamColors {
        primary: primary.to_string(),
        secondary: secondary.to_string(),
    })
}

/// A stored play style. The world keeps the generator's `{:?}` spelling, and a code this game does
/// not know keeps `Team::new`'s balanced default rather than becoming a made-up style.
fn play_style_from_code(code: &str) -> Option<PlayStyle> {
    match code.trim() {
        "Balanced" => Some(PlayStyle::Balanced),
        "Attacking" => Some(PlayStyle::Attacking),
        "Defensive" => Some(PlayStyle::Defensive),
        "Possession" => Some(PlayStyle::Possession),
        "Counter" => Some(PlayStyle::Counter),
        "HighPress" => Some(PlayStyle::HighPress),
        _ => None,
    }
}

fn footedness_from_code(code: &str) -> Option<Footedness> {
    match code.trim() {
        "Left" => Some(Footedness::Left),
        "Right" => Some(Footedness::Right),
        "Both" => Some(Footedness::Both),
        _ => None,
    }
}

/// A stored rating, when the source gave one this scale can carry.
fn rating(value: Option<i64>) -> Option<u8> {
    let value = value?;
    if !(1..=99).contains(&value) {
        return None;
    }
    u8::try_from(value).ok()
}

fn positive_u8(value: i64) -> Option<u8> {
    let value = u8::try_from(value).ok()?;
    (value > 0).then_some(value)
}

fn non_negative_u32(value: Option<i64>) -> Option<u32> {
    u32::try_from(value?).ok()
}

fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}
