//! The input side of the world database: a flat, id-referenced description of a world.
//!
//! A [`WorldSnapshot`] is what the importer writes and what tests build, without opening a
//! database. [`WorldSnapshot::from_world_data`] converts the generator's in-memory world into
//! one; a package import or the future live-sync layer builds one the same way.
//!
//! The in-memory `Game` model is deliberately not imported: a player there carries match state,
//! offers, media and a contract ledger, none of which belongs in a read-mostly world identity
//! table. Only the stable fields are copied.

use std::collections::{HashMap, HashSet};

use domain::player::Footedness;
use ofm_core::generator::WorldData;
use ofm_core::world_query::{fold_name, position_code, position_group_code};

use crate::world_db::WorldDbError;

/// A club in the world.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClubRecord {
    pub id: String,
    pub name: String,
    pub short_name: String,
    /// The football nation the club plays in (`ENG`, `SCO`, ...), not its physical location.
    pub country: String,
    pub city: String,
    pub stadium_name: String,
    pub stadium_capacity: Option<u32>,
    pub reputation: Option<u32>,
    pub founded_year: Option<u32>,
    /// Authored kit colours as JSON, when the source had them.
    pub colors_json: Option<String>,
    pub play_style: Option<String>,
}

/// A player's stable identity and ability, without match state or contract state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayerRecord {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub full_name: String,
    pub match_name: String,
    /// An exact position code from `world_query::position_code`.
    pub position: String,
    /// A group code from `world_query::position_group_code`.
    pub position_group: String,
    pub birth_date: Option<String>,
    pub birth_year: Option<i32>,
    pub nationality: String,
    pub football_nation: String,
    pub current_club_id: Option<String>,
    /// `None` when the source did not rate the player — never a stand-in zero.
    pub overall: Option<u8>,
    pub potential: Option<u8>,
    pub footedness: Option<String>,
    pub weak_foot: Option<u8>,
    /// The attribute block as JSON object text.
    pub attributes_json: String,
    pub retired: bool,
}

/// One season of a player's statistics in one competition. Every figure is optional: a source
/// that did not report minutes leaves them unknown, not zero.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SeasonStatRecord {
    pub player_id: String,
    pub season: u32,
    /// Empty for the player's whole season across competitions.
    pub competition_id: String,
    pub club_id: Option<String>,
    pub appearances: Option<u32>,
    pub minutes_played: Option<u32>,
    pub goals: Option<u32>,
    pub assists: Option<u32>,
    pub clean_sheets: Option<u32>,
    pub yellow_cards: Option<u32>,
    pub red_cards: Option<u32>,
    pub avg_rating: Option<f32>,
}

/// A club membership over a validity interval.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MembershipRecord {
    pub player_id: String,
    /// `None` when the club is known by name only.
    pub club_id: Option<String>,
    pub club_name: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub kind: String,
}

/// A proposed or confirmed link between a provider's identity and a local entity.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntityLinkRecord {
    pub provider: String,
    pub provider_id: String,
    pub entity_kind: String,
    pub entity_id: String,
    pub confidence: Option<f64>,
    pub reviewer: Option<String>,
    pub as_of: String,
}

/// One provider request, for resumable sync and de-duplication.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchLogRecord {
    pub provider: String,
    pub endpoint: String,
    pub status: Option<i64>,
    pub fetched_at: String,
    pub payload_hash: Option<String>,
}

/// A complete world ready to be imported: clubs, players, their history and the staging tables.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldSnapshot {
    pub clubs: Vec<ClubRecord>,
    pub players: Vec<PlayerRecord>,
    pub season_stats: Vec<SeasonStatRecord>,
    pub memberships: Vec<MembershipRecord>,
    pub entity_links: Vec<EntityLinkRecord>,
    pub fetch_log: Vec<FetchLogRecord>,
}

impl WorldSnapshot {
    /// Convert a generated (or loaded) world.
    ///
    /// `current_season` is the calendar year the world opens in; when given, each player's
    /// current-season totals (`Player::stats`) and per-competition league rows are imported under
    /// it. It is a parameter rather than read from the world because a generated world carries no
    /// opening season of its own, and guessing one would date rows wrongly.
    pub fn from_world_data(
        world: &WorldData,
        current_season: Option<u32>,
    ) -> Result<Self, WorldDbError> {
        let club_names: HashMap<&str, &str> = world
            .teams
            .iter()
            .map(|team| (team.id.as_str(), team.name.as_str()))
            .collect();

        let clubs = world
            .teams
            .iter()
            .map(club_record)
            .collect::<Result<Vec<_>, _>>()?;
        let mut players = Vec::with_capacity(world.players.len());
        let mut history = PlayerHistoryRows::default();

        for player in &world.players {
            players.push(player_record(player)?);
            history.push_career(player, &club_names);
            if let Some(season) = current_season {
                history.push_current_season_stats(player, season);
            }
            history.push_open_membership(player, &club_names);
        }

        Ok(Self {
            clubs,
            players,
            season_stats: history.season_stats,
            memberships: history.memberships,
            entity_links: Vec::new(),
            fetch_log: Vec::new(),
        })
    }
}

/// The per-player rows `from_world_data` accumulates, with the keys already used for stats.
#[derive(Default)]
struct PlayerHistoryRows {
    season_stats: Vec<SeasonStatRecord>,
    memberships: Vec<MembershipRecord>,
    seen_stats: HashSet<(String, u32, String)>,
}

impl PlayerHistoryRows {
    /// One stat row and one membership per career entry.
    fn push_career(&mut self, player: &domain::player::Player, club_names: &HashMap<&str, &str>) {
        for entry in &player.career {
            // A career row can name a club this world does not define (an authored history,
            // or a club that has since left the database). Its name is still worth keeping;
            // its id cannot satisfy the foreign key, so it stays unknown.
            let club_id = (!entry.team_id.is_empty()
                && club_names.contains_key(entry.team_id.as_str()))
            .then(|| entry.team_id.clone());
            push_stat(
                &mut self.season_stats,
                &mut self.seen_stats,
                SeasonStatRecord {
                    player_id: player.id.clone(),
                    season: entry.season,
                    competition_id: String::new(),
                    club_id: club_id.clone(),
                    appearances: Some(entry.appearances),
                    minutes_played: None,
                    goals: Some(entry.goals),
                    assists: Some(entry.assists),
                    clean_sheets: None,
                    yellow_cards: None,
                    red_cards: None,
                    avg_rating: None,
                },
            );
            self.memberships.push(MembershipRecord {
                player_id: player.id.clone(),
                club_id,
                club_name: entry.team_name.clone(),
                valid_from: entry.from.clone(),
                valid_to: entry.until.clone(),
                kind: String::from("career"),
            });
        }
    }

    /// The whole-season totals row plus one row per competition, all under `season`.
    fn push_current_season_stats(&mut self, player: &domain::player::Player, season: u32) {
        let current_club_id = player.team_id.clone();
        push_stat(
            &mut self.season_stats,
            &mut self.seen_stats,
            season_record(
                &player.id,
                &player.stats,
                season,
                String::new(),
                current_club_id.clone(),
            ),
        );
        for (competition_id, stats) in &player.league_stats_by_competition {
            push_stat(
                &mut self.season_stats,
                &mut self.seen_stats,
                season_record(
                    &player.id,
                    stats,
                    season,
                    competition_id.clone(),
                    current_club_id.clone(),
                ),
            );
        }
    }

    /// A `current` membership, unless the career already ends in an open stint at that club.
    fn push_open_membership(
        &mut self,
        player: &domain::player::Player,
        club_names: &HashMap<&str, &str>,
    ) {
        let Some(club_id) = &player.team_id else {
            return;
        };
        let already_open = player
            .career
            .last()
            .is_some_and(|entry| &entry.team_id == club_id && entry.until.is_none());
        if !already_open {
            self.memberships.push(MembershipRecord {
                player_id: player.id.clone(),
                club_id: Some(club_id.clone()),
                club_name: club_names
                    .get(club_id.as_str())
                    .map_or_else(String::new, |name| (*name).to_string()),
                valid_from: player.contract_start().map(str::to_string),
                valid_to: None,
                kind: String::from("current"),
            });
        }
    }
}

/// Keep the first row for a `(player, season, competition)` key. A player's `career` can already
/// hold the season `current_season` names (it is written at the rollover, and a mid-season move
/// splits it), and the table's primary key would otherwise reject the world one row later.
fn push_stat(
    rows: &mut Vec<SeasonStatRecord>,
    seen: &mut HashSet<(String, u32, String)>,
    row: SeasonStatRecord,
) {
    let key = (
        row.player_id.clone(),
        row.season,
        row.competition_id.clone(),
    );
    if seen.insert(key) {
        rows.push(row);
    }
}

fn club_record(team: &domain::team::Team) -> Result<ClubRecord, WorldDbError> {
    Ok(ClubRecord {
        id: team.id.clone(),
        name: team.name.clone(),
        short_name: team.short_name.clone(),
        country: if team.football_nation.is_empty() {
            team.country.clone()
        } else {
            team.football_nation.clone()
        },
        city: team.city.clone(),
        stadium_name: team.stadium_name.clone(),
        stadium_capacity: Some(team.stadium_capacity),
        reputation: Some(team.reputation),
        founded_year: Some(team.founded_year),
        colors_json: Some(
            serde_json::to_string(&team.colors)
                .map_err(|error| WorldDbError::Encode(error.to_string()))?,
        ),
        play_style: Some(format!("{:?}", team.play_style)),
    })
}

fn player_record(player: &domain::player::Player) -> Result<PlayerRecord, WorldDbError> {
    Ok(PlayerRecord {
        id: player.id.clone(),
        // The in-memory player keeps one display name; a package-authored first/last pair can
        // fill these later without changing the table.
        first_name: String::new(),
        last_name: String::new(),
        full_name: player.full_name.clone(),
        match_name: player.match_name.clone(),
        position: position_code(&player.position).to_string(),
        position_group: position_group_code(&player.position.to_group_position()).to_string(),
        birth_date: non_empty(&player.date_of_birth),
        birth_year: player
            .date_of_birth
            .get(..4)
            .and_then(|year| year.parse::<i32>().ok()),
        nationality: player.nationality.clone(),
        football_nation: player.football_nation.clone(),
        current_club_id: player.team_id.clone(),
        overall: Some(player.ovr).filter(|rating| *rating > 0),
        potential: Some(player.potential).filter(|rating| *rating > 0),
        footedness: Some(footedness_code(player.footedness).to_string()),
        weak_foot: Some(player.weak_foot),
        attributes_json: serde_json::to_string(&player.attributes)
            .map_err(|error| WorldDbError::Encode(error.to_string()))?,
        retired: player.retired,
    })
}

fn season_record(
    player_id: &str,
    stats: &domain::player::PlayerSeasonStats,
    season: u32,
    competition_id: String,
    club_id: Option<String>,
) -> SeasonStatRecord {
    SeasonStatRecord {
        player_id: player_id.to_string(),
        season,
        competition_id,
        club_id,
        appearances: Some(stats.appearances),
        minutes_played: Some(stats.minutes_played),
        goals: Some(stats.goals),
        assists: Some(stats.assists),
        clean_sheets: Some(stats.clean_sheets),
        yellow_cards: Some(stats.yellow_cards),
        red_cards: Some(stats.red_cards),
        // A 0.0 from nobody having rated a match is missing, not a real average of nought.
        avg_rating: (stats.rated_appearances > 0).then_some(stats.avg_rating),
    }
}

fn footedness_code(footedness: Footedness) -> &'static str {
    match footedness {
        Footedness::Left => "Left",
        Footedness::Right => "Right",
        Footedness::Both => "Both",
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// The case-folded form stored beside every name, so the importer and a query agree on one rule.
pub(crate) fn normalized_name(name: &str) -> String {
    fold_name(name)
}
