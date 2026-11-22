//! M3 acceptance tests for the career <-> `world.db` bridge.
//!
//! The point of the working set is that a career's load does not follow the size of the world: the
//! same twenty-club competition, read from a thousand-player file and from a hundred-thousand-player
//! file, must produce the same clubs, the same players and the same career save. Everything else the
//! world holds is reachable only on demand.
//!
//! The large world is assembled directly as a [`WorldSnapshot`]: a known, cheap, deterministic
//! hundred thousand rows that only differ from the small one outside the working set. Both paths meet
//! at the same importer the production world uses.

use std::time::Instant;

use chrono::{NaiveDate, TimeZone, Utc};
use domain::manager::Manager;
use domain::player::{Footedness, Position};
use ofm_core::clock::GameClock;
use ofm_core::game::Game;
use ofm_core::world_query::{position_code, position_group_code};

use crate::game_database::GameDatabase;
use crate::game_persistence::GamePersistenceWriter;

use super::*;

const FIRST_NAMES: [&str; 8] = [
    "Lionel",
    "Şükrü",
    "Ødegaard",
    "Kevin",
    "Marco",
    "Ali",
    "Sven",
    "Zinedine",
];
const LAST_NAMES: [&str; 8] = [
    "Messi",
    "Yılmaz",
    "Berg",
    "De Bruyne",
    "Rossi",
    "Demir",
    "Nilsson",
    "Zidane",
];
const NATIONS: [&str; 6] = ["ENG", "ES", "TR", "BR", "DE", "FR"];
const PLAY_STYLES: [&str; 6] = [
    "Balanced",
    "Attacking",
    "Defensive",
    "Possession",
    "Counter",
    "HighPress",
];
const FOOTEDNESS: [&str; 3] = ["Right", "Left", "Both"];

const PLAYERS_PER_CLUB: usize = 20;
const COMPETITION_CLUBS: usize = 20;
const COMPETITION_ID: &str = "comp-premier";
const SEASON: i32 = 2024;

/// `2.0x` against a save that may take three milliseconds is mostly timer noise. The floor keeps a
/// fast baseline from failing the assertion while still catching a save that actually grew.
const TIMING_FLOOR_MILLIS: f64 = 10.0;

fn season_start() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 7, 1).expect("a fixed date")
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A deterministic world of `club_count * PLAYERS_PER_CLUB` players.
///
/// Every club and player row depends only on its own club index and slot, never on how many clubs
/// the world holds, so the same twenty clubs are byte-identical in the 1k and 100k files and only
/// the clubs beyond them differ. Slot 0 and slot 10 of every club are free agents (so the free-agent
/// page is deterministic too), and slot 0 carries a deliberately incomplete attribute block.
fn synthetic_world(club_count: usize) -> WorldSnapshot {
    assert!(
        club_count >= COMPETITION_CLUBS,
        "a world needs the competition"
    );

    let clubs = (0..club_count).map(club_record).collect();
    let mut players = Vec::with_capacity(club_count * PLAYERS_PER_CLUB);
    for club in 0..club_count {
        for slot in 0..PLAYERS_PER_CLUB {
            players.push(player_record(club, slot));
        }
    }

    WorldSnapshot {
        clubs,
        players,
        season_stats: Vec::new(),
        memberships: Vec::new(),
        entity_links: Vec::new(),
        fetch_log: Vec::new(),
    }
}

fn club_record(index: usize) -> ClubRecord {
    ClubRecord {
        id: format!("club-{index:04}"),
        name: format!("Club {index:04}"),
        short_name: format!("C{index:03}"),
        country: NATIONS[index % NATIONS.len()].to_string(),
        city: format!("City {index}"),
        stadium_name: format!("Stadium {index}"),
        stadium_capacity: Some(10_000 + (index % 40) as u32 * 500),
        reputation: Some(300 + (index % 600) as u32),
        founded_year: Some(1880 + (index % 120) as u32),
        colors_json: Some(String::from(
            r##"{"primary":"#112233","secondary":"#ffffff"}"##,
        )),
        play_style: Some(PLAY_STYLES[index % PLAY_STYLES.len()].to_string()),
    }
}

fn player_record(club: usize, slot: usize) -> PlayerRecord {
    let position = Position::ALL[(club + slot) % Position::ALL.len()].clone();
    let overall = 40 + ((club * 3 + slot) % 60) as u8;
    let birth_year = 1985 + ((club + slot) % 20) as i32;
    // Slot 0 states one attribute and nothing else, so the fallback path is on the working set's
    // own deterministic conversion, not only on a hand-built fixture.
    let attributes_json = match slot {
        0 => String::from(r#"{"pace":70}"#),
        _ => format!(
            r#"{{"pace":{overall},"stamina":{},"strength":{},"passing":{},"shooting":{},"tackling":{},"dribbling":{},"defending":{},"positioning":{},"vision":{},"decisions":{}}}"#,
            overall - 2,
            overall + 1,
            overall - 4,
            overall + 2,
            overall - 6,
            overall + 3,
            overall - 3,
            overall - 1,
            overall + 4,
            overall
        ),
    };

    PlayerRecord {
        id: format!("player-{club:04}-{slot:02}"),
        first_name: FIRST_NAMES[slot % FIRST_NAMES.len()].to_string(),
        last_name: LAST_NAMES[(club + slot) % LAST_NAMES.len()].to_string(),
        full_name: format!(
            "{} {} {club}{slot}",
            FIRST_NAMES[slot % FIRST_NAMES.len()],
            LAST_NAMES[(club + slot) % LAST_NAMES.len()]
        ),
        match_name: LAST_NAMES[(club + slot) % LAST_NAMES.len()].to_string(),
        position: position_code(&position).to_string(),
        position_group: position_group_code(&position.to_group_position()).to_string(),
        birth_date: Some(format!("{birth_year}-06-15")),
        birth_year: Some(birth_year),
        nationality: NATIONS[(club * 3 + slot) % NATIONS.len()].to_string(),
        football_nation: String::from("ENG"),
        current_club_id: (!slot.is_multiple_of(10)).then(|| format!("club-{club:04}")),
        overall: Some(overall),
        potential: Some((overall + 3).min(99)),
        footedness: Some(FOOTEDNESS[(club + slot) % FOOTEDNESS.len()].to_string()),
        weak_foot: Some((1 + (club + slot) % 5) as u8),
        attributes_json,
        retired: false,
    }
}

/// A world with no optional statistics at all: no rating, no potential, no foot, no birth date and
/// one attribute. What the bridge derives must stay in memory, not become a row value.
fn sparse_world() -> WorldSnapshot {
    let club = ClubRecord {
        id: String::from("club-0000"),
        name: String::from("Sparse FC"),
        short_name: String::from("SFC"),
        country: String::from("ENG"),
        city: String::from("Town"),
        stadium_name: String::from("Ground"),
        stadium_capacity: None,
        reputation: None,
        founded_year: None,
        colors_json: None,
        play_style: None,
    };
    let sparse = PlayerRecord {
        id: String::from("sparse"),
        full_name: String::from("Sparse Player"),
        match_name: String::from("Sparse"),
        position: String::from("Striker"),
        position_group: String::from("forward"),
        nationality: String::from("ENG"),
        football_nation: String::from("ENG"),
        current_club_id: Some(String::from("club-0000")),
        attributes_json: String::from(r#"{"pace":70}"#),
        ..PlayerRecord::default()
    };
    let rated = PlayerRecord {
        id: String::from("rated"),
        overall: Some(80),
        potential: Some(85),
        ..sparse.clone()
    };

    WorldSnapshot {
        clubs: vec![club],
        players: vec![sparse, rated],
        ..WorldSnapshot::default()
    }
}

/// The world the career is playing in: `players` players spread over as many clubs as that needs,
/// with [`COMPETITION_CLUBS`] of them members of one competition in [`SEASON`].
fn world_with_working_set(players: usize) -> WorldDatabase {
    let mut world = WorldDatabase::open_in_memory().expect("an in-memory world");
    world
        .import(&synthetic_world(players / PLAYERS_PER_CLUB))
        .expect("the synthetic world imports");
    let sync = world.sync();
    sync.upsert_competition(&CompetitionRow {
        id: String::from(COMPETITION_ID),
        name: String::from("Premier Division"),
        short_name: String::from("PRM"),
        country: Some(String::from("ENG")),
        tier: 1,
        kind: String::from("league"),
        confederation: None,
        updated_at: "2026-07-01T00:00:00Z".parse().expect("a fixed timestamp"),
    })
    .expect("the competition is stored");
    for club in 0..COMPETITION_CLUBS {
        sync.upsert_competition_club(&CompetitionClubRow {
            competition_id: String::from(COMPETITION_ID),
            club_id: format!("club-{club:04}"),
            season_year: i64::from(SEASON),
        })
        .expect("the membership is stored");
    }
    world
}

fn working_set_request() -> WorkingSetRequest {
    WorkingSetRequest {
        competition_ids: vec![String::from(COMPETITION_ID)],
        extra_club_ids: Vec::new(),
        season_year: Some(SEASON),
        include_free_agents_limit: Some(5),
    }
}

fn player_fingerprints(players: &[domain::player::Player]) -> Vec<String> {
    players
        .iter()
        .map(|player| serde_json::to_string(player).expect("a player serialises"))
        .collect()
}

fn club_fingerprints(clubs: &[domain::team::Team]) -> Vec<String> {
    clubs
        .iter()
        .map(|club| serde_json::to_string(club).expect("a club serialises"))
        .collect()
}

// ---------------------------------------------------------------------------
// Working-set isolation
// ---------------------------------------------------------------------------

/// Given the same twenty-club competition in a thousand-player world and a hundred-thousand-player
///       world,
/// When each career loads its working set,
/// Then both load the same clubs and players and only `total_world_players` differs.
#[test]
fn a_working_set_is_the_same_in_a_thousand_and_a_hundred_thousand_player_world() {
    let request = working_set_request();
    let small_world = world_with_working_set(1_000);
    let large_world = world_with_working_set(100_000);

    let small =
        load_working_set(&small_world, &request, season_start()).expect("the small set loads");
    let large =
        load_working_set(&large_world, &request, season_start()).expect("the large set loads");

    assert_eq!(small.total_world_players, 1_000);
    assert_eq!(large.total_world_players, 100_000);
    assert_eq!(small.clubs.len(), COMPETITION_CLUBS);
    assert_eq!(large.clubs.len(), COMPETITION_CLUBS);
    assert_eq!(small.loaded_club_ids, large.loaded_club_ids);
    assert_eq!(small.players.len(), large.players.len());
    assert_eq!(
        player_fingerprints(&small.players),
        player_fingerprints(&large.players)
    );
    assert_eq!(
        club_fingerprints(&small.clubs),
        club_fingerprints(&large.clubs)
    );
    assert!(
        small.players.len() < 500,
        "a working set of {} players is not bounded",
        small.players.len()
    );
    assert!(
        small.players.len() < 100_000 / 100,
        "the working set should be a small fraction of the world"
    );
}

/// Given a request that names a club outside the competition,
/// When its working set is loaded,
/// Then the extra club is there, and a club the world never defined is simply absent.
#[test]
fn a_requested_extra_club_joins_the_working_set_and_an_unknown_one_is_absent() {
    let world = world_with_working_set(1_000);
    let request = WorkingSetRequest {
        extra_club_ids: vec![String::from("club-0035"), String::from("club-9999")],
        ..working_set_request()
    };

    let bundle = load_working_set(&world, &request, season_start()).expect("the set loads");

    assert!(bundle.loaded_club_ids.contains(&String::from("club-0035")));
    assert!(!bundle.loaded_club_ids.contains(&String::from("club-9999")));
    assert_eq!(bundle.loaded_club_ids.len(), COMPETITION_CLUBS + 1);
    assert!(
        bundle
            .players
            .iter()
            .any(|player| player.team_id.as_deref() == Some("club-0035"))
    );
}

// ---------------------------------------------------------------------------
// Materialisation outside the working set
// ---------------------------------------------------------------------------

/// Given players a career has not loaded — a free agent and a player of a club outside the
///       competition,
/// When each is materialised by id,
/// Then he is the player the world holds, and an id the world never had is `None`.
#[test]
fn a_player_outside_the_working_set_materialises_on_demand() {
    let world = world_with_working_set(1_000);
    let bundle =
        load_working_set(&world, &working_set_request(), season_start()).expect("set loads");

    let free_agent = materialize_player(&world, "player-0030-00", season_start())
        .expect("the free agent reads")
        .expect("the free agent exists");
    assert_eq!(free_agent.id, "player-0030-00");
    assert!(free_agent.team_id.is_none());
    assert!(
        !bundle
            .players
            .iter()
            .any(|player| player.id == free_agent.id)
    );

    let external = materialize_player(&world, "player-0035-05", season_start())
        .expect("the external player reads")
        .expect("the external player exists");
    assert_eq!(external.team_id.as_deref(), Some("club-0035"));
    assert!(!bundle.players.iter().any(|player| player.id == external.id));

    assert!(
        materialize_player(&world, "no-such-player", season_start())
            .expect("a missing id is not an error")
            .is_none()
    );
    assert!(
        materialize_player(&world, "   ", season_start())
            .expect("a blank id is not an error")
            .is_none()
    );
}

/// Given a club outside the working set,
/// When its squad is materialised,
/// Then it holds exactly the players that club owns, and a club the world does not define is empty.
#[test]
fn a_club_outside_the_working_set_materialises_its_own_squad() {
    let world = world_with_working_set(1_000);

    let squad =
        materialize_club_squad(&world, "club-0035", season_start()).expect("the squad reads");

    assert_eq!(squad.len(), PLAYERS_PER_CLUB - 2);
    assert!(
        squad
            .iter()
            .all(|player| player.team_id.as_deref() == Some("club-0035"))
    );
    assert!(
        squad
            .iter()
            .all(|player| player.id.starts_with("player-0035-"))
    );
    assert!(
        materialize_club_squad(&world, "club-9999", season_start())
            .expect("an unknown club is not an error")
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// Missing statistics stay missing
// ---------------------------------------------------------------------------

/// Given a player whose world row states no rating, potential, foot, weak foot or birth date and
///       only one attribute,
/// When he is materialised,
/// Then the career still gets a coherent player, and the world row is still NULL afterwards —
///      nothing derived is written back.
#[test]
fn missing_statistics_are_derived_in_memory_and_never_written_back() {
    let mut world = WorldDatabase::open_in_memory().expect("an in-memory world");
    world
        .import(&sparse_world())
        .expect("the sparse world imports");

    let player = materialize_player(&world, "sparse", season_start())
        .expect("the player reads")
        .expect("the player exists");

    assert!(player.ovr > 0, "an unstated rating must not read as zero");
    assert!(player.potential >= player.ovr);
    assert_eq!(player.footedness, Footedness::Right);
    assert!(player.date_of_birth.is_empty());
    assert_eq!(player.attributes.pace, 70);
    assert_eq!(
        player.attributes.passing, player.attributes.stamina,
        "every unstated attribute falls back to the same neutral value, not to zero"
    );
    assert!(player.attributes.passing > 0);
    assert_eq!(player.team_id.as_deref(), Some("club-0000"));

    let rated = materialize_player(&world, "rated", season_start())
        .expect("the rated player reads")
        .expect("the rated player exists");
    assert_eq!(rated.ovr, 80);
    assert_eq!(rated.potential, 85);
    assert_eq!(
        rated.attributes.passing, 80,
        "a missing field falls back to the stated overall"
    );

    let stored = stored_statistics(&world, "sparse");
    assert_eq!(stored.overall, None, "overall must stay NULL");
    assert_eq!(stored.potential, None, "potential must stay NULL");
    assert_eq!(stored.footedness, None, "footedness must stay NULL");
    assert_eq!(stored.weak_foot, None, "weak foot must stay NULL");
    assert_eq!(stored.birth_date, None, "birth date must stay NULL");
    assert_eq!(
        stored.attributes_json, r#"{"pace":70}"#,
        "the attribute block must not be rewritten"
    );
}

/// The sparse row as SQLite still holds it.
struct StoredStatistics {
    overall: Option<i64>,
    potential: Option<i64>,
    footedness: Option<String>,
    weak_foot: Option<i64>,
    birth_date: Option<String>,
    attributes_json: String,
}

fn stored_statistics(world: &WorldDatabase, player_id: &str) -> StoredStatistics {
    world
        .conn()
        .query_row(
            "SELECT overall, potential, footedness, weak_foot, birth_date, attributes_json \
             FROM world_players WHERE id = ?1",
            [player_id],
            |row| {
                Ok(StoredStatistics {
                    overall: row.get(0)?,
                    potential: row.get(1)?,
                    footedness: row.get(2)?,
                    weak_foot: row.get(3)?,
                    birth_date: row.get(4)?,
                    attributes_json: row.get(5)?,
                })
            },
        )
        .expect("the world row reads")
}

// ---------------------------------------------------------------------------
// Save-time decoupling
// ---------------------------------------------------------------------------

struct SaveMeasurement {
    millis: f64,
    attempt_millis: Vec<f64>,
    load_millis: f64,
    bytes: u64,
    teams: i64,
    players: i64,
}

struct SaveRun {
    millis: f64,
    bytes: u64,
    teams: i64,
    players: i64,
}

/// Load a working set, turn it into a career, save it to a fresh file and measure the save.
///
/// The world is opened fresh and the set loaded *outside* the save timer: the benchmark is about
/// what a save costs, not about how fast a hundred thousand rows import.
fn measure_save(world: &WorldDatabase, request: &WorkingSetRequest) -> SaveMeasurement {
    let load_started = Instant::now();
    let bundle = load_working_set(world, request, season_start()).expect("the set loads");
    let load_millis = elapsed_millis(load_started);
    let game = game_from_bundle(&bundle);

    // One untimed write absorbs first-touch costs (allocator, journal setup, OS cache). Without it
    // the world measured first always looks slowest, and the ratio would be measuring process warm-up
    // rather than the save itself.
    let warm_up = save_once(&game);
    let first = save_once(&game);
    let second = save_once(&game);
    let attempt_millis = vec![warm_up.millis, first.millis, second.millis];
    let best = if second.millis <= first.millis {
        second
    } else {
        first
    };

    SaveMeasurement {
        millis: best.millis,
        attempt_millis,
        load_millis,
        bytes: best.bytes,
        teams: best.teams,
        players: best.players,
    }
}

/// Write a career to its own fresh SQLite file, and time the write alone.
fn save_once(game: &Game) -> SaveRun {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("career.db");
    let database = GameDatabase::open(&path).expect("a career database opens");

    let started = Instant::now();
    GamePersistenceWriter::write_game(&database, game, "save-1", "Career")
        .expect("the career saves");
    let millis = elapsed_millis(started);

    let teams = count_rows(&database, "teams");
    let players = count_rows(&database, "players");
    drop(database);
    let bytes = std::fs::metadata(&path)
        .expect("the save file exists")
        .len();

    SaveRun {
        millis,
        bytes,
        teams,
        players,
    }
}

fn elapsed_millis(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn count_rows(database: &GameDatabase, table: &str) -> i64 {
    database
        .conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("the table is countable")
}

fn game_from_bundle(bundle: &WorkingSetBundle) -> Game {
    let manager = Manager::new(
        String::from("mgr-1"),
        String::from("Alex"),
        String::from("Manager"),
        String::from("1980-01-01"),
        String::from("England"),
    );
    let clock = GameClock::new(
        Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0)
            .single()
            .expect("a fixed start date"),
    );
    Game::new(
        clock,
        manager,
        bundle.clubs.clone(),
        bundle.players.clone(),
        Vec::new(),
        Vec::new(),
    )
}

/// Given a career built from the same twenty-club competition in a thousand-player world and in a
///       hundred-thousand-player world,
/// When each is written to its own SQLite career save,
/// Then the saves hold the same teams and players and are the same size, and the hundred-thousand
///      save is not materially slower than the thousand one — the world's extra players never reach
///      the save path at all.
#[test]
fn a_career_save_from_a_working_set_does_not_grow_with_the_world() {
    let request = working_set_request();
    let small_world = world_with_working_set(1_000);
    let large_world = world_with_working_set(100_000);

    let baseline = measure_save(&small_world, &request);
    let large = measure_save(&large_world, &request);

    println!(
        "M3 save benchmark: 1k-backed working set — save runs {:?} ms (best {:.2}), load {:.2} ms; \
         100k-backed — save runs {:?} ms (best {:.2}), load {:.2} ms; \
         {} vs {} bytes, {} vs {} teams, {} vs {} players",
        baseline.attempt_millis,
        baseline.millis,
        baseline.load_millis,
        large.attempt_millis,
        large.millis,
        large.load_millis,
        baseline.bytes,
        large.bytes,
        baseline.teams,
        large.teams,
        baseline.players,
        large.players
    );

    assert_eq!(baseline.teams, large.teams);
    assert_eq!(baseline.players, large.players);
    assert_eq!(
        baseline.bytes, large.bytes,
        "the career save must not carry the size of the world it was read from"
    );
    assert!(baseline.teams > 0 && baseline.players > 0);

    let allowed_millis = (baseline.millis * 2.0).max(baseline.millis + TIMING_FLOOR_MILLIS);
    assert!(
        large.millis < allowed_millis,
        "a 100k-backed save took {:.2} ms against a {:.2} ms baseline (allowed {:.2} ms)",
        large.millis,
        baseline.millis,
        allowed_millis
    );
}
