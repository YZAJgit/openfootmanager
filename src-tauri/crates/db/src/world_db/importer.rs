//! Bulk insert of a [`WorldSnapshot`] into `world.db`.
//!
//! One transaction and one prepared statement per table: a hundred thousand players ride the
//! same three statements, so the cost is the rows themselves rather than per-row preparation.
//! The import is a *replacement*, not an append — see `WorldDatabase::import`.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, Transaction, params};

use crate::world_db::snapshot::normalized_name;
use crate::world_db::{
    ClubRecord, EntityLinkRecord, FetchLogRecord, MembershipRecord, PlayerRecord, SeasonStatRecord,
    WorldDbError, WorldSnapshot,
};

/// What one import wrote, so a caller can log or assert on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportSummary {
    pub clubs: usize,
    pub players: usize,
    pub season_stats: usize,
    pub memberships: usize,
    pub entity_links: usize,
    pub fetch_log: usize,
}

/// Replace every world row with `snapshot`, atomically.
pub(super) fn import(
    conn: &mut Connection,
    snapshot: &WorldSnapshot,
) -> Result<ImportSummary, WorldDbError> {
    validate(snapshot)?;

    let transaction = conn.transaction()?;
    clear_world(&transaction)?;
    insert_clubs(&transaction, &snapshot.clubs)?;

    let club_names: HashMap<&str, &str> = snapshot
        .clubs
        .iter()
        .map(|club| (club.id.as_str(), club.name.as_str()))
        .collect();
    insert_players(&transaction, &snapshot.players, &club_names)?;
    insert_memberships(&transaction, &snapshot.memberships)?;
    insert_season_stats(&transaction, &snapshot.season_stats)?;
    insert_entity_links(&transaction, &snapshot.entity_links)?;
    insert_fetch_log(&transaction, &snapshot.fetch_log)?;
    transaction.commit()?;
    checkpoint(conn)?;

    Ok(ImportSummary {
        clubs: snapshot.clubs.len(),
        players: snapshot.players.len(),
        season_stats: snapshot.season_stats.len(),
        memberships: snapshot.memberships.len(),
        entity_links: snapshot.entity_links.len(),
        fetch_log: snapshot.fetch_log.len(),
    })
}

/// Truncate the write-ahead log this import wrote, so a read-mostly world file does not carry the
/// whole import behind it until some later open happens to checkpoint. Runs after the commit:
/// the world is already stored, and a busy answer only means a live reader held pages, so the WAL
/// simply waits for the next checkpoint instead of anything being wrong with the import.
fn checkpoint(conn: &Connection) -> Result<(), WorldDbError> {
    let (_busy, _wal, _checkpointed): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    Ok(())
}

/// Every id a row refers to must be in the snapshot, and no id may be declared twice. Catching it
/// here names the offending id; letting it reach SQLite would only say "constraint failed".
///
/// Staging rows (`entity_links`, `fetch_log`) are the exception: they are stored as given, because
/// their ids name provider entities the live-sync layer — not a world snapshot — is responsible
/// for resolving and for choosing the canonical link.
fn validate(snapshot: &WorldSnapshot) -> Result<(), WorldDbError> {
    let mut club_ids: HashSet<&str> = HashSet::with_capacity(snapshot.clubs.len());
    for club in &snapshot.clubs {
        if club.id.is_empty() {
            return Err(WorldDbError::InvalidSnapshot(String::from(
                "a club has no id",
            )));
        }
        if !club_ids.insert(club.id.as_str()) {
            return Err(WorldDbError::InvalidSnapshot(format!(
                "club `{}` is declared twice",
                club.id
            )));
        }
    }

    let mut player_ids: HashSet<&str> = HashSet::with_capacity(snapshot.players.len());
    for player in &snapshot.players {
        if player.id.is_empty() {
            return Err(WorldDbError::InvalidSnapshot(String::from(
                "a player has no id",
            )));
        }
        if !player_ids.insert(player.id.as_str()) {
            return Err(WorldDbError::InvalidSnapshot(format!(
                "player `{}` is declared twice",
                player.id
            )));
        }
        if let Some(club_id) = &player.current_club_id
            && !club_ids.contains(club_id.as_str())
        {
            return Err(WorldDbError::InvalidSnapshot(format!(
                "player `{}` refers to unknown club `{club_id}`",
                player.id
            )));
        }
    }

    let mut stat_keys: HashSet<(&str, u32, &str)> =
        HashSet::with_capacity(snapshot.season_stats.len());
    for stat in &snapshot.season_stats {
        if !player_ids.contains(stat.player_id.as_str()) {
            return Err(WorldDbError::InvalidSnapshot(format!(
                "a season statistic refers to unknown player `{}`",
                stat.player_id
            )));
        }
        if let Some(club_id) = &stat.club_id
            && !club_ids.contains(club_id.as_str())
        {
            return Err(WorldDbError::InvalidSnapshot(format!(
                "a season statistic refers to unknown club `{club_id}`"
            )));
        }
        if !stat_keys.insert((
            stat.player_id.as_str(),
            stat.season,
            stat.competition_id.as_str(),
        )) {
            return Err(WorldDbError::InvalidSnapshot(format!(
                "player `{}` declares season `{}` in competition `{}` twice",
                stat.player_id, stat.season, stat.competition_id
            )));
        }
    }

    for membership in &snapshot.memberships {
        if !player_ids.contains(membership.player_id.as_str()) {
            return Err(WorldDbError::InvalidSnapshot(format!(
                "a membership period refers to unknown player `{}`",
                membership.player_id
            )));
        }
        if let Some(club_id) = &membership.club_id
            && !club_ids.contains(club_id.as_str())
        {
            return Err(WorldDbError::InvalidSnapshot(format!(
                "a membership period refers to unknown club `{club_id}`"
            )));
        }
    }

    Ok(())
}

fn clear_world(transaction: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    // Children before parents, even though the foreign keys cascade, so the intent is readable.
    transaction.execute("DELETE FROM world_player_search", [])?;
    transaction.execute("DELETE FROM world_player_season_stats", [])?;
    transaction.execute("DELETE FROM world_membership_periods", [])?;
    transaction.execute("DELETE FROM world_players", [])?;
    transaction.execute("DELETE FROM world_clubs", [])?;
    transaction.execute("DELETE FROM entity_links", [])?;
    transaction.execute("DELETE FROM fetch_log", [])?;
    Ok(())
}

fn insert_clubs(
    transaction: &Transaction<'_>,
    clubs: &[ClubRecord],
) -> Result<(), rusqlite::Error> {
    let mut statement = transaction.prepare(
        "INSERT INTO world_clubs (
            id, name, short_name, country, city, stadium_name, stadium_capacity, reputation,
            founded_year, colors_json, play_style, name_normalized
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for club in clubs {
        statement.execute(params![
            club.id,
            club.name,
            club.short_name,
            club.country,
            club.city,
            club.stadium_name,
            club.stadium_capacity,
            club.reputation,
            club.founded_year,
            club.colors_json,
            club.play_style,
            normalized_name(&club.name),
        ])?;
    }
    Ok(())
}

fn insert_players(
    transaction: &Transaction<'_>,
    players: &[PlayerRecord],
    club_names: &HashMap<&str, &str>,
) -> Result<(), rusqlite::Error> {
    let mut insert = transaction.prepare(
        "INSERT INTO world_players (
            id, first_name, last_name, full_name, match_name, name_normalized, position,
            position_group, birth_date, birth_year, nationality, football_nation, current_club_id,
            overall, potential, footedness, weak_foot, attributes_json, retired
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
    )?;
    let mut search = transaction.prepare(
        "INSERT INTO world_player_search (player_id, name, club_name) VALUES (?1, ?2, ?3)",
    )?;
    for player in players {
        insert.execute(params![
            player.id,
            player.first_name,
            player.last_name,
            player.full_name,
            player.match_name,
            normalized_name(&player.full_name),
            player.position,
            player.position_group,
            player.birth_date,
            player.birth_year,
            player.nationality,
            player.football_nation,
            player.current_club_id,
            player.overall,
            player.potential,
            player.footedness,
            player.weak_foot,
            player.attributes_json,
            player.retired,
        ])?;

        let club_name = player
            .current_club_id
            .as_deref()
            .and_then(|club_id| club_names.get(club_id).copied())
            .unwrap_or_default();
        search.execute(params![
            player.id,
            normalized_name(&player.full_name),
            normalized_name(club_name),
        ])?;
    }
    Ok(())
}

fn insert_memberships(
    transaction: &Transaction<'_>,
    memberships: &[MembershipRecord],
) -> Result<(), rusqlite::Error> {
    let mut statement = transaction.prepare(
        "INSERT INTO world_membership_periods (
            player_id, club_id, club_name, valid_from, valid_to, kind
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for membership in memberships {
        statement.execute(params![
            membership.player_id,
            membership.club_id,
            membership.club_name,
            membership.valid_from,
            membership.valid_to,
            membership.kind,
        ])?;
    }
    Ok(())
}

fn insert_season_stats(
    transaction: &Transaction<'_>,
    season_stats: &[SeasonStatRecord],
) -> Result<(), rusqlite::Error> {
    let mut statement = transaction.prepare(
        "INSERT INTO world_player_season_stats (
            player_id, season, competition_id, club_id, appearances, minutes_played, goals,
            assists, clean_sheets, yellow_cards, red_cards, avg_rating
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for stat in season_stats {
        statement.execute(params![
            stat.player_id,
            stat.season,
            stat.competition_id,
            stat.club_id,
            stat.appearances,
            stat.minutes_played,
            stat.goals,
            stat.assists,
            stat.clean_sheets,
            stat.yellow_cards,
            stat.red_cards,
            stat.avg_rating,
        ])?;
    }
    Ok(())
}

fn insert_entity_links(
    transaction: &Transaction<'_>,
    links: &[EntityLinkRecord],
) -> Result<(), rusqlite::Error> {
    let mut statement = transaction.prepare(
        "INSERT INTO entity_links (
            provider, provider_id, entity_kind, entity_id, confidence, reviewer, as_of
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for link in links {
        statement.execute(params![
            link.provider,
            link.provider_id,
            link.entity_kind,
            link.entity_id,
            link.confidence,
            link.reviewer,
            link.as_of,
        ])?;
    }
    Ok(())
}

fn insert_fetch_log(
    transaction: &Transaction<'_>,
    entries: &[FetchLogRecord],
) -> Result<(), rusqlite::Error> {
    let mut statement = transaction.prepare(
        "INSERT INTO fetch_log (provider, endpoint, status, fetched_at, payload_hash)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for entry in entries {
        statement.execute(params![
            entry.provider,
            entry.endpoint,
            entry.status,
            entry.fetched_at,
            entry.payload_hash,
        ])?;
    }
    Ok(())
}
