//! World-database tests: migrations, import idempotency, paged queries, NULL semantics, index
//! plans and a hundred-thousand-player import.
//!
//! The large world is assembled directly as a [`WorldSnapshot`] rather than through the
//! generator: the generator's realistic spread is exercised by the compact-world test, while this
//! one needs a known, cheap hundred thousand rows. Both paths meet at the same importer.

use std::time::Instant;

use chrono::{DateTime, Utc};
use domain::player::Position;
use ofm_core::generator::definitions::DefinitionSources;
use ofm_core::generator::{WorldGenConfig, generate_world_data_seeded_with};
use ofm_core::world_query::{
    Page, PlayerKeysetCursor, PlayerSearch, PlayerSort, PlayerSortColumn, PlayerSortValue,
    PlayerSummary, PositionFilter, fold_name,
};
use ofm_core::world_sync::{CanonicalLink, SyncMode, SyncRequestPlan, plan_request, prune_cutoff};
use rusqlite::params_from_iter;

use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

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

/// A deterministic world with a known distribution: players go round-robin through the clubs
/// (every tenth is a free agent), positions through [`Position::ALL`], nations through a six-code
/// cycle, and ratings through 40..=99.
fn synthetic_snapshot(club_count: usize, player_count: usize) -> WorldSnapshot {
    assert!(club_count > 0, "a world needs at least one club");

    let clubs = (0..club_count)
        .map(|index| ClubRecord {
            id: format!("club-{index:03}"),
            name: if index % 7 == 0 {
                format!("Malmö FF {index:03}")
            } else {
                format!("Club {index:03}")
            },
            short_name: format!("C{index:02}"),
            country: NATIONS[index % NATIONS.len()].to_string(),
            city: format!("City {index}"),
            stadium_name: format!("Stadium {index}"),
            stadium_capacity: Some(10_000 + index as u32 * 10),
            reputation: Some(500 + (index % 400) as u32),
            founded_year: Some(1900 + (index % 100) as u32),
            colors_json: Some(String::from(
                r##"{"primary":"#112233","secondary":"#ffffff"}"##,
            )),
            play_style: Some(String::from("Balanced")),
        })
        .collect();

    let mut players = Vec::with_capacity(player_count);
    let mut season_stats = Vec::with_capacity(player_count * 2);
    let mut memberships = Vec::with_capacity(player_count);

    for index in 0..player_count {
        let first = FIRST_NAMES[index % FIRST_NAMES.len()];
        let last = LAST_NAMES[(index / FIRST_NAMES.len()) % LAST_NAMES.len()];
        let position = Position::ALL[index % Position::ALL.len()].clone();
        let nationality = NATIONS[index % NATIONS.len()].to_string();
        let birth_year = 1985 + (index % 20) as i32;
        let overall = 40 + (index % 60) as u8;
        let club_id = (index % 10 != 0).then(|| format!("club-{:03}", index % club_count));

        players.push(PlayerRecord {
            id: format!("player-{index:06}"),
            first_name: first.to_string(),
            last_name: last.to_string(),
            full_name: format!("{first} {last} {index}"),
            match_name: last.to_string(),
            position: ofm_core::world_query::position_code(&position).to_string(),
            position_group: ofm_core::world_query::position_group_code(
                &position.to_group_position(),
            )
            .to_string(),
            birth_date: Some(format!("{birth_year}-06-15")),
            birth_year: Some(birth_year),
            nationality,
            football_nation: String::from("ENG"),
            current_club_id: club_id.clone(),
            overall: Some(overall),
            potential: Some((overall + 3).min(99)),
            footedness: Some(String::from("Right")),
            weak_foot: Some((index % 5) as u8 + 1),
            attributes_json: String::from(r#"{"pace":70,"passing":65,"shooting":60}"#),
            retired: false,
        });

        season_stats.push(SeasonStatRecord {
            player_id: format!("player-{index:06}"),
            season: 2024,
            competition_id: String::new(),
            club_id: club_id.clone(),
            appearances: Some((index % 30) as u32),
            minutes_played: Some((index % 2_500) as u32),
            goals: Some((index % 12) as u32),
            assists: Some((index % 8) as u32),
            clean_sheets: None,
            yellow_cards: Some((index % 6) as u32),
            red_cards: Some(u32::from(index % 23 == 0)),
            avg_rating: Some(6.5),
        });
        // Every other player's most recent season has an appearances count and nothing else, so
        // a test can prove the missing figures stay NULL rather than becoming zero.
        season_stats.push(SeasonStatRecord {
            player_id: format!("player-{index:06}"),
            season: 2025,
            competition_id: String::new(),
            club_id: club_id.clone(),
            appearances: Some((index % 5) as u32),
            minutes_played: None,
            goals: None,
            assists: None,
            clean_sheets: None,
            yellow_cards: None,
            red_cards: None,
            avg_rating: None,
        });

        if let Some(club_id) = club_id {
            memberships.push(MembershipRecord {
                player_id: format!("player-{index:06}"),
                club_id: Some(club_id.clone()),
                club_name: format!("Club {:03}", index % club_count),
                valid_from: None,
                valid_to: None,
                kind: String::from("current"),
            });
        }
    }

    WorldSnapshot {
        clubs,
        players,
        season_stats,
        memberships,
        entity_links: Vec::new(),
        fetch_log: Vec::new(),
    }
}

/// Every row of every world table, as strings, sorted so row order and page layout cannot hide a
/// difference. Used to prove a second import of one snapshot changes nothing.
fn dump_world(db: &WorldDatabase) -> Vec<(String, Vec<String>)> {
    const TABLES: [&str; 7] = [
        "world_clubs",
        "world_players",
        "world_player_season_stats",
        "world_membership_periods",
        "world_player_search",
        "entity_links",
        "fetch_log",
    ];
    TABLES
        .iter()
        .map(|table| {
            let mut statement = db
                .conn()
                .prepare(&format!("SELECT * FROM {table}"))
                .expect("a world table should be readable");
            let columns = statement.column_count();
            let mut rows: Vec<String> = statement
                .query_map([], |row| {
                    let mut values = Vec::with_capacity(columns);
                    for index in 0..columns {
                        values.push(format!("{:?}", row.get_ref(index)?));
                    }
                    Ok(values.join("|"))
                })
                .expect("the world table should be queryable")
                .collect::<Result<_, _>>()
                .expect("every row should read");
            rows.sort();
            (String::from(*table), rows)
        })
        .collect()
}

fn count_rows(db: &WorldDatabase, table: &str) -> i64 {
    db.conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("table should be countable")
}

/// `EXPLAIN QUERY PLAN` details for a statement, so a test asserts on the plan SQLite actually
/// uses rather than on the SQL's prose.
fn query_plan(db: &WorldDatabase, sql: &str, params: &[rusqlite::types::Value]) -> Vec<String> {
    let mut statement = db
        .conn()
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("the query should be explainable");
    statement
        .query_map(params_from_iter(params.iter()), |row| {
            row.get::<_, String>(3)
        })
        .expect("the plan should be queryable")
        .collect::<Result<_, _>>()
        .expect("every plan row should read")
}

/// A page of players must never come from a full scan of the players or clubs tables. An index
/// scan is allowed (`SCAN p USING INDEX ...`), a bare `SCAN p` is not.
fn assert_no_full_table_scan(plan: &[String], alias: &str) {
    for line in plan {
        let scans_alias = line.contains(&format!("SCAN {alias}"));
        assert!(
            !scans_alias || line.contains("USING") || line.contains("VIRTUAL TABLE"),
            "`{alias}` was scanned in full: {line}\nplan: {plan:#?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Schema and migration round-trip
// ---------------------------------------------------------------------------

/// Given an empty path
/// When a world database is opened, closed and opened again
/// Then it reports the pinned schema version both times and its tables survive the round trip.
#[test]
fn a_world_file_migrates_once_and_reopens_at_the_same_version() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join(WORLD_DATABASE_FILE);

    {
        let db = WorldDatabase::open(&path).expect("a fresh world file should open");
        assert_eq!(
            db.schema_version().expect("version readable"),
            WORLD_MIGRATION_COUNT as i64
        );
        let journal_mode: String = db
            .conn()
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("journal mode readable");
        assert_eq!(journal_mode, "wal", "a world file is written in WAL");
    }

    let reopened = WorldDatabase::open(&path).expect("an existing world file should reopen");
    assert_eq!(
        reopened.schema_version().expect("version readable"),
        WORLD_MIGRATION_COUNT as i64,
        "reopening must not apply a migration twice"
    );
    for table in [
        "world_clubs",
        "world_players",
        "world_player_season_stats",
        "world_membership_periods",
        "entity_links",
        "fetch_log",
    ] {
        assert_eq!(
            count_rows(&reopened, table),
            0,
            "{table} should exist and be empty"
        );
    }
}

/// Given the bundled SQLite build
/// When a world database is opened
/// Then the FTS5 player search table exists and answers a query.
#[test]
fn the_bundled_sqlite_has_fts5_and_the_search_table() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let tables: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'world_player_search'",
            [],
            |row| row.get(0),
        )
        .expect("sqlite_master should be readable");
    assert_eq!(tables, 1, "world_player_search should be an FTS5 table");
    assert_eq!(count_rows(&db, "world_player_search"), 0);
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Given a world generated by the existing generator
/// When it is imported and read back
/// Then every club and player is there, a squad page matches the source, and the profile carries
///      the player's current membership and current-season rows.
#[test]
fn a_generated_world_imports_with_its_clubs_players_and_history() {
    let world = generate_world_data_seeded_with(
        7,
        &WorldGenConfig::compact(),
        &DefinitionSources::embedded_only(),
    );
    let snapshot =
        WorldSnapshot::from_world_data(&world, Some(2026)).expect("the world should convert");
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");

    let summary = db.import(&snapshot).expect("import should succeed");

    assert_eq!(summary.clubs, world.teams.len());
    assert_eq!(summary.players, world.players.len());
    assert_eq!(count_rows(&db, "world_clubs"), world.teams.len() as i64);
    assert_eq!(count_rows(&db, "world_players"), world.players.len() as i64);
    assert_eq!(
        count_rows(&db, "world_player_search"),
        world.players.len() as i64,
        "every player is searchable"
    );

    let team = world.teams.first().expect("the generator makes clubs");
    let squad = db
        .club_squad_page(&team.id, None, Page::first(100))
        .expect("a squad page should read");
    let expected_squad = world
        .players
        .iter()
        .filter(|player| player.team_id.as_deref() == Some(team.id.as_str()))
        .count() as u64;
    assert_eq!(squad.total, expected_squad);
    assert!(
        squad.players.iter().all(|player| {
            player.current_club_id.as_deref() == Some(team.id.as_str())
                && player.current_club_name.as_deref() == Some(team.name.as_str())
        }),
        "a squad page must hold only that club's players"
    );

    let player = world
        .players
        .iter()
        .find(|player| player.team_id.is_some())
        .expect("the generator makes attached players");
    let profile = db
        .get_player_profile(&player.id)
        .expect("the profile should read")
        .expect("the player should exist");
    assert_eq!(profile.player.id, player.id);
    assert!(
        profile
            .memberships
            .iter()
            .any(|period| period.kind == "current"),
        "a player with a club has a current membership: {:?}",
        profile.memberships
    );
    assert!(
        profile.season_stats.iter().any(|line| line.season == 2026),
        "the requested current season was imported: {:?}",
        profile.season_stats
    );
    assert!(
        profile.attributes_json.starts_with('{'),
        "attributes are stored as a JSON object"
    );
}

/// Given a world snapshot
/// When the same snapshot is imported twice
/// Then every table holds the same rows (no duplicates), FTS included.
#[test]
fn reimporting_the_same_world_changes_no_rows() {
    let snapshot = synthetic_snapshot(5, 40);
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");

    db.import(&snapshot).expect("first import should succeed");
    let first = dump_world(&db);
    let summary = db.import(&snapshot).expect("second import should succeed");
    let second = dump_world(&db);

    assert_eq!(summary.players, 40);
    assert_eq!(first, second, "a re-import is a replacement, not an append");
    assert_eq!(count_rows(&db, "world_players"), 40);
    assert_eq!(count_rows(&db, "world_player_search"), 40);
}

/// Given a snapshot whose player refers to a club it does not carry
/// When it is imported
/// Then it is refused naming the player and club, and the database is left untouched.
#[test]
fn an_orphaned_player_is_refused_without_writing() {
    let mut snapshot = synthetic_snapshot(1, 1);
    snapshot.players[0].current_club_id = Some(String::from("ghost-club"));
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&synthetic_snapshot(1, 3))
        .expect("the valid world should import");

    let error = db
        .import(&snapshot)
        .expect_err("an orphan must be refused")
        .to_string();

    assert!(error.contains("ghost-club"), "{error}");
    assert_eq!(
        count_rows(&db, "world_players"),
        3,
        "the refused import must not clear the previous world"
    );
}

/// Given a snapshot carrying staging rows
/// When it is imported and read back
/// Then the link and the fetch log row are stored with their nullable columns intact.
#[test]
fn entity_links_and_fetch_log_round_trip() {
    let mut snapshot = synthetic_snapshot(1, 1);
    snapshot.entity_links.push(EntityLinkRecord {
        provider: String::from("api-football"),
        provider_id: String::from("42"),
        entity_kind: String::from("player"),
        entity_id: String::from("player-000000"),
        confidence: Some(0.75),
        reviewer: Some(String::from("importer")),
        as_of: String::from("2026-11-19"),
    });
    snapshot.fetch_log.push(FetchLogRecord {
        provider: String::from("api-football"),
        endpoint: String::from("/players?id=42"),
        status: Some(200),
        fetched_at: String::from("2026-11-19T10:00:00Z"),
        payload_hash: None,
    });
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");

    db.import(&snapshot).expect("import should succeed");

    let (provider, confidence, reviewer, as_of): (String, Option<f64>, Option<String>, String) = db
        .conn()
        .query_row(
            "SELECT provider, confidence, reviewer, as_of FROM entity_links",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("the link should be stored");
    assert_eq!(
        (
            provider.as_str(),
            confidence,
            reviewer.as_deref(),
            as_of.as_str()
        ),
        ("api-football", Some(0.75), Some("importer"), "2026-11-19")
    );
    let (status, hash): (Option<i64>, Option<String>) = db
        .conn()
        .query_row("SELECT status, payload_hash FROM fetch_log", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("the fetch log row should be stored");
    assert_eq!((status, hash), (Some(200), None));
}

// ---------------------------------------------------------------------------
// Paged queries
// ---------------------------------------------------------------------------

/// Given a world with known clubs, positions, nations and ratings
/// When players are searched with each filter
/// Then every row satisfies the filter, the total is the size of the filtered set, and the sort
///      is the requested one.
#[test]
fn player_pages_filter_sort_and_page_correctly() {
    let snapshot = synthetic_snapshot(4, 80);
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&snapshot).expect("import should succeed");

    let everything = db
        .search_players_page(&PlayerSearch::default(), Page::first(10))
        .expect("an unfiltered page should read");
    assert_eq!(everything.total, 80);
    assert_eq!(everything.players.len(), 10);
    assert!(
        everything
            .players
            .windows(2)
            .all(|pair| pair[0].overall >= pair[1].overall),
        "the default order is overall descending"
    );

    let second = db
        .search_players_page(&PlayerSearch::default(), Page::new(10, 10))
        .expect("the second page should read");
    assert!(
        everything
            .players
            .iter()
            .all(|first| second.players.iter().all(|later| later.id != first.id)),
        "pages must not overlap"
    );

    let club = PlayerSearch {
        club_id: Some(String::from("club-001")),
        ..PlayerSearch::default()
    };
    let club_page = db
        .search_players_page(&club, Page::first(200))
        .expect("a club page should read");
    let expected_club = snapshot
        .players
        .iter()
        .filter(|player| player.current_club_id.as_deref() == Some("club-001"))
        .count() as u64;
    assert!(expected_club > 0);
    assert_eq!(club_page.total, expected_club);
    assert!(
        club_page
            .players
            .iter()
            .all(|player| player.current_club_id.as_deref() == Some("club-001"))
    );

    let forwards = PlayerSearch {
        position: Some(PositionFilter::group(Position::Forward)),
        ..PlayerSearch::default()
    };
    let forward_page = db
        .search_players_page(&forwards, Page::first(200))
        .expect("a position group page should read");
    assert_eq!(
        forward_page.total,
        snapshot
            .players
            .iter()
            .filter(|player| player.position_group == "forward")
            .count() as u64
    );
    assert!(
        forward_page
            .players
            .iter()
            .all(|player| player.position_group == "forward")
    );

    let strikers = PlayerSearch {
        position: Some(PositionFilter::exact(Position::Striker)),
        ..PlayerSearch::default()
    };
    let striker_page = db
        .search_players_page(&strikers, Page::first(200))
        .expect("an exact position page should read");
    assert!(striker_page.total > 0);
    assert!(
        striker_page
            .players
            .iter()
            .all(|player| player.position == "Striker"),
        "an exact position must not widen to its group"
    );

    let turks = PlayerSearch {
        nationality: Some(String::from("TR")),
        ..PlayerSearch::default()
    };
    let turkish_page = db
        .search_players_page(&turks, Page::first(200))
        .expect("a nationality page should read");
    let expected_turks = snapshot
        .players
        .iter()
        .filter(|player| player.nationality == "TR")
        .count() as u64;
    assert!(expected_turks > 0);
    assert_eq!(turkish_page.total, expected_turks);
    assert!(
        turkish_page
            .players
            .iter()
            .all(|player| player.nationality == "TR")
    );

    let band = PlayerSearch {
        overall_min: Some(70),
        overall_max: Some(79),
        ..PlayerSearch::default()
    };
    let band_page = db
        .search_players_page(&band, Page::first(200))
        .expect("a rating band should read");
    assert!(band_page.total > 0);
    assert!(
        band_page
            .players
            .iter()
            .all(|player| (70..=79).contains(&player.overall.expect("rated"))),
        "every row is inside the requested band"
    );

    let by_name = PlayerSearch {
        sort: PlayerSort::ascending(PlayerSortColumn::Name),
        ..PlayerSearch::default()
    };
    let by_name_page = db
        .search_players_page(&by_name, Page::first(80))
        .expect("a name-sorted page should read");
    assert!(
        by_name_page
            .players
            .windows(2)
            .all(|pair| fold_name(&pair[0].full_name) <= fold_name(&pair[1].full_name)),
        "name order is the folded name order"
    );
}

/// Given a world holding a Turkish-named player and a Swedish club
/// When searched by a diacritic-free name or by club name
/// Then FTS5 finds them.
#[test]
fn name_search_matches_folded_player_and_club_names() {
    let snapshot = synthetic_snapshot(8, 80);
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&snapshot).expect("import should succeed");

    let by_player = PlayerSearch {
        name: Some(String::from("sukru")),
        ..PlayerSearch::default()
    };
    let player_page = db
        .search_players_page(&by_player, Page::first(200))
        .expect("a name search should read");
    assert_eq!(player_page.total, 10, "one in eight first names is Şükrü");
    assert!(
        player_page
            .players
            .iter()
            .all(|player| player.full_name.contains("Şükrü"))
    );

    let by_club = PlayerSearch {
        name: Some(String::from("malmo")),
        ..PlayerSearch::default()
    };
    let club_page = db
        .search_players_page(&by_club, Page::first(200))
        .expect("a club-name search should read");
    assert!(club_page.total > 0);
    assert!(club_page.players.iter().all(|player| {
        player
            .current_club_name
            .as_deref()
            .is_some_and(|name| name.starts_with("Malmö"))
    }));
}

/// Given a club's squad
/// When paged with and without a position filter
/// Then the page is that club's players in overall order, narrowed by the filter.
#[test]
fn club_squads_page_and_narrow_by_position() {
    let snapshot = synthetic_snapshot(3, 60);
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&snapshot).expect("import should succeed");

    let squad = db
        .club_squad_page("club-000", None, Page::first(200))
        .expect("a squad page should read");
    let expected = snapshot
        .players
        .iter()
        .filter(|player| player.current_club_id.as_deref() == Some("club-000"))
        .count() as u64;
    assert_eq!(squad.total, expected);
    assert!(
        squad
            .players
            .windows(2)
            .all(|pair| pair[0].overall >= pair[1].overall)
    );

    let keepers = db
        .club_squad_page(
            "club-000",
            Some(PositionFilter::group(Position::Goalkeeper)),
            Page::first(200),
        )
        .expect("a filtered squad page should read");
    assert!(keepers.total > 0);
    assert!(
        keepers
            .players
            .iter()
            .all(|player| player.position_group == "goalkeeper")
    );
}

// ---------------------------------------------------------------------------
// NULL statistics
// ---------------------------------------------------------------------------

/// Given a player with a fully reported season and a partially reported one
/// When the profile is read
/// Then the unreported figures are absent (`None`), not zero, and no season was invented.
#[test]
fn missing_statistics_read_back_as_missing_not_zero() {
    let mut snapshot = synthetic_snapshot(1, 1);
    snapshot.players[0].current_club_id = Some(String::from("club-000"));
    snapshot.memberships = vec![MembershipRecord {
        player_id: String::from("player-000000"),
        club_id: Some(String::from("club-000")),
        club_name: String::from("Club 000"),
        valid_from: Some(String::from("2024-07-01")),
        valid_to: None,
        kind: String::from("current"),
    }];
    snapshot.season_stats = vec![
        SeasonStatRecord {
            player_id: String::from("player-000000"),
            season: 2024,
            competition_id: String::new(),
            club_id: Some(String::from("club-000")),
            appearances: Some(30),
            minutes_played: Some(2_700),
            goals: Some(12),
            assists: Some(4),
            clean_sheets: Some(9),
            yellow_cards: Some(3),
            red_cards: Some(1),
            avg_rating: Some(7.1),
        },
        SeasonStatRecord {
            player_id: String::from("player-000000"),
            season: 2025,
            competition_id: String::new(),
            club_id: Some(String::from("club-000")),
            appearances: Some(4),
            minutes_played: None,
            goals: None,
            assists: None,
            clean_sheets: None,
            yellow_cards: None,
            red_cards: None,
            avg_rating: None,
        },
    ];
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&snapshot).expect("import should succeed");

    let profile = db
        .get_player_profile("player-000000")
        .expect("profile should read")
        .expect("player should exist");

    assert_eq!(profile.season_stats.len(), 2, "no season was invented");
    assert_eq!(profile.season_stats[0].season, 2025, "newest first");
    assert_eq!(profile.season_stats[0].appearances, Some(4));
    assert_eq!(profile.season_stats[0].goals, None);
    assert_eq!(profile.season_stats[0].minutes_played, None);
    assert_eq!(profile.season_stats[0].avg_rating, None);
    assert_eq!(profile.season_stats[1].goals, Some(12));
    assert_eq!(profile.season_stats[1].avg_rating, Some(7.1));
    assert_eq!(
        profile.memberships.len(),
        1,
        "the current membership is kept"
    );
    assert_eq!(
        profile.memberships[0].valid_to, None,
        "the current club is open-ended"
    );
}

// ---------------------------------------------------------------------------
// Index plans
// ---------------------------------------------------------------------------

/// Given the statements the query layer builds
/// When SQLite explains them
/// Then each main filter is served by its index and neither players nor clubs are fully scanned.
#[test]
fn the_main_filters_are_index_backed() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");

    let cases: Vec<(&str, PlayerSearch, &str)> = vec![
        (
            "club",
            PlayerSearch {
                club_id: Some(String::from("club-001")),
                ..PlayerSearch::default()
            },
            "idx_world_players_club_overall",
        ),
        (
            "position group",
            PlayerSearch {
                position: Some(PositionFilter::group(Position::Forward)),
                ..PlayerSearch::default()
            },
            "idx_world_players_position_group",
        ),
        (
            "exact position",
            PlayerSearch {
                position: Some(PositionFilter::exact(Position::Striker)),
                ..PlayerSearch::default()
            },
            "idx_world_players_position",
        ),
        (
            "nationality",
            PlayerSearch {
                nationality: Some(String::from("TR")),
                ..PlayerSearch::default()
            },
            "idx_world_players_nationality",
        ),
        (
            "rating band",
            PlayerSearch {
                overall_min: Some(70),
                overall_max: Some(79),
                ..PlayerSearch::default()
            },
            "idx_world_players_overall",
        ),
    ];

    for (label, search, expected_index) in cases {
        let (sql, params) = crate::world_db::queries::test_support::search_sql(&search);
        let plan = query_plan(&db, &sql, &params);
        assert_no_full_table_scan(&plan, "p");
        assert_no_full_table_scan(&plan, "c");
        assert!(
            plan.iter().any(|line| line.contains(expected_index)),
            "the {label} filter should use {expected_index}, plan: {plan:#?}"
        );

        let (count_sql, count_params) = crate::world_db::queries::test_support::count_sql(&search);
        let count_plan = query_plan(&db, &count_sql, &count_params);
        assert_no_full_table_scan(&count_plan, "p");
    }
}

/// Given a name search
/// When SQLite explains it
/// Then the FTS5 table drives the query rather than the players table being scanned.
#[test]
fn a_name_search_drives_from_the_fts_index() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let search = PlayerSearch {
        name: Some(String::from("sukru yilmaz")),
        ..PlayerSearch::default()
    };

    let (sql, params) = crate::world_db::queries::test_support::search_sql(&search);
    let plan = query_plan(&db, &sql, &params);

    assert_no_full_table_scan(&plan, "p");
    assert!(
        plan.iter()
            .any(|line| line.contains("world_player_search") || line.contains("VIRTUAL TABLE")),
        "the FTS table should appear in the plan: {plan:#?}"
    );

    let (count_sql, count_params) = crate::world_db::queries::test_support::count_sql(&search);
    let count_plan = query_plan(&db, &count_sql, &count_params);
    assert_no_full_table_scan(&count_plan, "p");
    assert!(
        count_plan
            .iter()
            .any(|line| line.contains("world_player_search") || line.contains("VIRTUAL TABLE")),
        "the count must use the FTS index too: {count_plan:#?}"
    );
}

// ---------------------------------------------------------------------------
// Sort coverage, replacement and staging validation
// ---------------------------------------------------------------------------

/// Given the eight unfiltered sort orders the browse contract exposes
/// When SQLite explains each page statement
/// Then every one is an index walk, with no sort of the table behind it.
#[test]
fn every_unfiltered_sort_is_index_backed() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let orders = [
        PlayerSort::ascending(PlayerSortColumn::Overall),
        PlayerSort::descending(PlayerSortColumn::Overall),
        PlayerSort::ascending(PlayerSortColumn::Potential),
        PlayerSort::descending(PlayerSortColumn::Potential),
        PlayerSort::ascending(PlayerSortColumn::Name),
        PlayerSort::descending(PlayerSortColumn::Name),
        PlayerSort::ascending(PlayerSortColumn::BirthYear),
        PlayerSort::descending(PlayerSortColumn::BirthYear),
    ];

    for sort in orders {
        let search = PlayerSearch {
            sort,
            ..PlayerSearch::default()
        };
        let (sql, params) = crate::world_db::queries::test_support::search_sql(&search);
        let plan = query_plan(&db, &sql, &params);
        assert_no_full_table_scan(&plan, "p");
        assert!(
            !plan.iter().any(|line| line.contains("USE TEMP B-TREE")),
            "{sort:?} must be an index walk, not a sort of any part of the table: {plan:#?}"
        );
    }
}

/// Given an imported world and a second, smaller snapshot
/// When the smaller world is imported
/// Then every player, club, statistic and search row of the first world is gone.
#[test]
fn a_shrinking_world_replaces_the_previous_one() {
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let first_world = synthetic_snapshot(4, 40);
    let gone_name = first_world.players[39].full_name.clone();
    db.import(&first_world)
        .expect("the first world should import");

    db.import(&synthetic_snapshot(1, 3))
        .expect("the smaller world should import");

    assert_eq!(count_rows(&db, "world_clubs"), 1);
    assert_eq!(count_rows(&db, "world_players"), 3);
    assert_eq!(count_rows(&db, "world_player_search"), 3);
    assert_eq!(count_rows(&db, "world_player_season_stats"), 6);
    assert!(
        db.get_player_profile("player-000039")
            .expect("the profile read should succeed")
            .is_none(),
        "a player only the first world carried is gone"
    );
    let old_name = PlayerSearch {
        name: Some(gone_name),
        ..PlayerSearch::default()
    };
    assert_eq!(
        db.search_players_page(&old_name, Page::first(10))
            .expect("the name search should read")
            .total,
        0,
        "the search index holds no first-world row"
    );
}

/// Given a snapshot that declares one player's season twice in the same competition
/// When it is imported
/// Then it is refused naming the player, season and competition, and the world in place stays.
#[test]
fn a_duplicate_season_stat_is_refused_with_its_key() {
    let mut snapshot = synthetic_snapshot(1, 1);
    let duplicate = snapshot.season_stats[0].clone();
    snapshot.season_stats.push(duplicate);
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&synthetic_snapshot(1, 3))
        .expect("the valid world should import");

    let error = db
        .import(&snapshot)
        .expect_err("the duplicate key must be refused")
        .to_string();

    assert!(error.contains("player-000000"), "{error}");
    assert!(error.contains("2024"), "{error}");
    assert_eq!(
        count_rows(&db, "world_players"),
        3,
        "the refused import must not clear the world in place"
    );
}

/// Given a file-backed world database
/// When a world is imported
/// Then the write-ahead log behind it is truncated, so a read-mostly file does not carry the
///      whole import until some later open.
#[test]
fn an_import_empties_the_wal_behind_it() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join(WORLD_DATABASE_FILE);
    let mut db = WorldDatabase::open(&path).expect("a world file should open");

    db.import(&synthetic_snapshot(2, 20))
        .expect("import should succeed");

    let wal = directory.path().join(format!("{WORLD_DATABASE_FILE}-wal"));
    let wal_length = std::fs::metadata(&wal).map(|meta| meta.len()).unwrap_or(0);
    assert_eq!(wal_length, 0, "the import must checkpoint its WAL away");
}

/// Given a snapshot whose players have an empty name and a blank one
/// When it is imported
/// Then both rows and their search entries survive, and the browse stays answerable.
#[test]
fn blank_names_import_and_stay_findable_by_id() {
    let mut snapshot = synthetic_snapshot(1, 2);
    snapshot.players[0].full_name = String::new();
    snapshot.players[1].full_name = String::from("   ");
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");

    db.import(&snapshot)
        .expect("blank names must not fail the import");

    assert_eq!(count_rows(&db, "world_player_search"), 2);
    assert!(
        db.get_player_profile("player-000000")
            .expect("the profile read should succeed")
            .is_some()
    );
    assert_eq!(
        db.search_players_page(&PlayerSearch::default(), Page::first(10))
            .expect("the browse should read")
            .total,
        2
    );
}

// ---------------------------------------------------------------------------
// Scale
// ---------------------------------------------------------------------------

/// Given a hundred thousand generated players and five hundred clubs
/// When the world is imported into a file-backed database
/// Then the import and several paged queries answer with correct rows inside a generous budget.
#[test]
fn a_hundred_thousand_player_world_imports_and_pages() {
    const CLUBS: usize = 500;
    const PLAYERS: usize = 100_000;

    let snapshot = synthetic_snapshot(CLUBS, PLAYERS);
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join(WORLD_DATABASE_FILE);
    let started = Instant::now();

    let mut db = WorldDatabase::open(&path).expect("a world file should open");
    let summary = db
        .import(&snapshot)
        .expect("the large import should succeed");
    let imported = started.elapsed();

    assert_eq!(summary.players, PLAYERS);
    assert_eq!(count_rows(&db, "world_players"), PLAYERS as i64);
    assert_eq!(
        count_rows(&db, "world_player_season_stats"),
        (PLAYERS * 2) as i64
    );
    assert_eq!(count_rows(&db, "world_player_search"), PLAYERS as i64);

    let squad = db
        .club_squad_page("club-001", None, Page::first(25))
        .expect("a club page should read");
    let expected_squad = snapshot
        .players
        .iter()
        .filter(|player| player.current_club_id.as_deref() == Some("club-001"))
        .count() as u64;
    assert!(expected_squad > 25);
    assert_eq!(squad.total, expected_squad);
    assert_eq!(squad.players.len(), 25);
    assert!(
        squad
            .players
            .windows(2)
            .all(|pair| pair[0].overall >= pair[1].overall)
    );

    let band = PlayerSearch {
        overall_min: Some(85),
        overall_max: Some(89),
        ..PlayerSearch::default()
    };
    let band_page = db
        .search_players_page(&band, Page::new(10, 20))
        .expect("a rating page should read");
    assert!(band_page.total > 0);
    assert_eq!(band_page.players.len(), 20);
    assert!(
        band_page
            .players
            .iter()
            .all(|player| (85..=89).contains(&player.overall.expect("rated"))),
        "every page row is inside the band"
    );
    assert!(
        band_page
            .players
            .windows(2)
            .all(|pair| pair[0].overall >= pair[1].overall)
    );

    let target = &snapshot.players[99_999];
    let by_name = PlayerSearch {
        name: Some(target.full_name.clone()),
        ..PlayerSearch::default()
    };
    let named = db
        .search_players_page(&by_name, Page::first(10))
        .expect("a name search should read");
    assert_eq!(named.total, 1, "one player carries that exact name");
    assert_eq!(named.players[0].id, target.id);

    let profile = db
        .get_player_profile("player-099999")
        .expect("profile should read")
        .expect("the player should exist");
    assert_eq!(profile.season_stats.len(), 2);

    println!(
        "100k world: imported {} players in {:?}; total {:?}",
        summary.players,
        imported,
        started.elapsed()
    );
    assert!(
        started.elapsed().as_secs() < 600,
        "a hundred thousand rows took {:?}; that is a pathological import",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// M2: schema version two
// ---------------------------------------------------------------------------

/// The w001 script, embedded the way the migration set embeds it, so a test can build a file that
/// looks like one an M1 build wrote.
const W001_SQL: &str = include_str!("../world_sql/w001_world_schema.sql");

/// Given a database opened from empty
/// When its schema is inspected
/// Then it is at version two and carries the tables and indexes w002 adds.
#[test]
fn the_world_schema_reaches_version_two_with_its_competition_and_sync_tables() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");

    assert_eq!(WORLD_MIGRATION_COUNT, 2);
    assert_eq!(
        db.schema_version().expect("version readable"),
        2,
        "w002 is the second migration"
    );
    for table in [
        "world_competitions",
        "world_competition_clubs",
        "world_sync_runs",
    ] {
        assert_eq!(
            count_rows(&db, table),
            0,
            "{table} should exist and be empty"
        );
    }
    for index in [
        "idx_world_competition_clubs_club",
        "idx_world_sync_runs_route",
    ] {
        let found: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [index],
                |row| row.get(0),
            )
            .expect("sqlite_master should be readable");
        assert_eq!(found, 1, "{index} should exist");
    }
}

/// Given a file written by an M1 build: w001 applied, `user_version = 1`, a club in place
/// When it is opened by this build
/// Then w002 applies, the club survives, and the new tables appear.
#[test]
fn a_version_one_world_file_upgrades_and_keeps_its_rows() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join(WORLD_DATABASE_FILE);
    {
        let conn = rusqlite::Connection::open(&path).expect("a legacy world file should open");
        conn.execute_batch(W001_SQL)
            .expect("the M1 schema should apply");
        conn.pragma_update(None, "user_version", 1)
            .expect("the M1 version should be pinned");
        conn.execute(
            "INSERT INTO world_clubs (id, name) VALUES ('club-legacy', 'Legacy FC')",
            [],
        )
        .expect("the legacy club should insert");
    }

    let db = WorldDatabase::open(&path).expect("an M1 world file should upgrade");
    assert_eq!(db.schema_version().expect("version readable"), 2);
    let legacy: String = db
        .conn()
        .query_row(
            "SELECT name FROM world_clubs WHERE id = 'club-legacy'",
            [],
            |row| row.get(0),
        )
        .expect("the legacy club should survive");
    assert_eq!(legacy, "Legacy FC", "w002 never rewrites a w001 row");
    assert_eq!(count_rows(&db, "world_competitions"), 0);
}

// ---------------------------------------------------------------------------
// M2: atomic read snapshots
// ---------------------------------------------------------------------------

/// Given a file-backed world, a reader holding one read snapshot and a writer that commits a
/// bigger world
/// When the count and the page are read through the held snapshot after the write
/// Then both still describe the world before it, while a fresh read sees the new one.
#[test]
fn a_held_read_snapshot_counts_and_pages_the_same_world() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join(WORLD_DATABASE_FILE);
    let mut writer = WorldDatabase::open(&path).expect("a world file should open");
    writer
        .import(&synthetic_snapshot(2, 20))
        .expect("the first world should import");
    let reader = WorldDatabase::open(&path).expect("a second connection should open");

    let search = PlayerSearch {
        club_id: Some(String::from("club-001")),
        ..PlayerSearch::default()
    };
    let (count_sql, count_params) = crate::world_db::queries::test_support::count_sql(&search);
    let (select_sql, select_params) = crate::world_db::queries::test_support::search_sql(&search);

    let snapshot = crate::world_db::queries::test_support::OpenSnapshot::begin(reader.conn())
        .expect("a read snapshot should begin");
    let count_in_snapshot: i64 = snapshot
        .conn()
        .query_row(&count_sql, params_from_iter(count_params.iter()), |row| {
            row.get(0)
        })
        .expect("the snapshot should count");

    writer
        .import(&synthetic_snapshot(2, 60))
        .expect("a bigger world should import");

    let rows_in_snapshot = {
        let mut statement = snapshot
            .conn()
            .prepare(&select_sql)
            .expect("the snapshot should prepare the page");
        statement
            .query_map(params_from_iter(select_params.iter()), |row| {
                row.get::<_, String>(0)
            })
            .expect("the snapshot should page")
            .collect::<Result<Vec<_>, _>>()
            .expect("every page row should read")
    };

    assert_eq!(
        count_in_snapshot,
        i64::try_from(rows_in_snapshot.len()).expect("a small count"),
        "count and page must see one snapshot"
    );
    assert_eq!(
        count_in_snapshot, 10,
        "the first world held ten players at club-001"
    );
    drop(snapshot);

    let after = reader
        .search_players_page(&search, Page::first(200))
        .expect("a fresh read should page");
    assert_eq!(after.total, 30, "a new snapshot sees the new world");
    assert_eq!(after.players.len(), 30);
}

// ---------------------------------------------------------------------------
// M2: keyset paging
// ---------------------------------------------------------------------------

/// An instant, for the sync rows, fixed so no test depends on the clock.
fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).expect("a representable instant")
}

/// A world with the three positions a keyset walk has to cross: tied sort values, NULL ratings and
/// unknown birth years, and players who share a name so the id tie-break is exercised.
fn tied_and_null_snapshot() -> WorldSnapshot {
    let mut players = Vec::with_capacity(100);
    for index in 0..100_i32 {
        let overall =
            (index % 5 != 0).then(|| 40 + u8::try_from(index / 3 % 5).unwrap_or_default());
        let birth_year = (index % 7 != 0).then(|| 1990 + index % 4);
        players.push(PlayerRecord {
            id: format!("p-{index:03}"),
            full_name: format!("Name {}", index % 3),
            position: String::from("Striker"),
            position_group: String::from("forward"),
            birth_date: birth_year.map(|year| format!("{year}-01-01")),
            birth_year,
            nationality: String::from("ENG"),
            current_club_id: Some(String::from("club-000")),
            overall,
            potential: overall.map(|rating| rating.saturating_add(1)),
            attributes_json: String::from("{}"),
            ..PlayerRecord::default()
        });
    }
    WorldSnapshot {
        clubs: vec![ClubRecord {
            id: String::from("club-000"),
            name: String::from("Club Zero"),
            ..ClubRecord::default()
        }],
        players,
        ..WorldSnapshot::default()
    }
}

fn every_player_order() -> Vec<PlayerSort> {
    let mut orders = Vec::with_capacity(8);
    for column in [
        PlayerSortColumn::Overall,
        PlayerSortColumn::Potential,
        PlayerSortColumn::Name,
        PlayerSortColumn::BirthYear,
    ] {
        orders.push(PlayerSort::ascending(column));
        orders.push(PlayerSort::descending(column));
    }
    orders
}

fn ids(page: &[PlayerSummary]) -> Vec<String> {
    page.iter().map(|player| player.id.clone()).collect()
}

/// The ids an `OFFSET` walk returns, page by page.
fn offset_walk(db: &WorldDatabase, search: &PlayerSearch, page_size: u32) -> Vec<String> {
    let mut seen = Vec::new();
    let mut offset = 0;
    loop {
        let page = db
            .search_players_page(search, Page::new(offset, page_size))
            .expect("an offset page should read");
        let short = page.players.len() < page_size as usize;
        seen.extend(ids(&page.players));
        offset += u64::from(page_size);
        if short {
            return seen;
        }
    }
}

/// The ids a keyset walk returns, page by page, via each page's `next_cursor`.
fn keyset_walk(db: &WorldDatabase, search: &PlayerSearch, page_size: u32) -> Vec<String> {
    let mut seen = Vec::new();
    let mut cursor: Option<PlayerKeysetCursor> = None;
    loop {
        let page = db
            .search_players_keyset(search, page_size, cursor.as_ref())
            .expect("a keyset page should read");
        seen.extend(ids(&page.players));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return seen,
        }
    }
}

/// Given a world with tied ratings, unrated players, unknown birth years and repeated names
/// When every one of the eight orders is walked both by `OFFSET` and by keyset
/// Then the two walks return exactly the same players in exactly the same order.
#[test]
fn keyset_paging_walks_the_same_order_as_offset_paging() {
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&tied_and_null_snapshot())
        .expect("the tied world should import");

    for sort in every_player_order() {
        let search = PlayerSearch {
            sort,
            ..PlayerSearch::default()
        };
        let by_offset = offset_walk(&db, &search, 7);
        let by_keyset = keyset_walk(&db, &search, 7);

        assert_eq!(by_keyset.len(), 100, "{sort:?} walked the whole world");
        assert_eq!(
            by_keyset, by_offset,
            "{sort:?} must page the same order either way"
        );
        let distinct: std::collections::BTreeSet<&String> = by_keyset.iter().collect();
        assert_eq!(distinct.len(), 100, "{sort:?} repeated a player");
    }
}

/// Given a world of a hundred rows and a page size of seven
/// When the keyset pages are read one after another
/// Then the first page carries the total, a seek page does not re-count, and each full page
///      reports a cursor while the last one reports none.
#[test]
fn a_keyset_page_reports_its_total_and_whether_the_set_continues() {
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&tied_and_null_snapshot())
        .expect("the tied world should import");
    let search = PlayerSearch::default();

    let first = db
        .search_players_keyset(&search, 7, None)
        .expect("the first page should read");
    assert_eq!(first.total, Some(100));
    assert_eq!(first.limit, 7);
    assert_eq!(first.players.len(), 7);
    assert!(first.next_cursor.is_some());

    let second = db
        .search_players_keyset(&search, 7, first.next_cursor.as_ref())
        .expect("the second page should read");
    assert_eq!(
        second.total, None,
        "a seek page must not pay for a count it already has"
    );

    let last = db
        .search_players_keyset(&search, 100, None)
        .expect("a whole-set page should read");
    assert_eq!(last.players.len(), 100);
    assert!(
        last.next_cursor.is_some(),
        "a page that filled its window may still have rows behind it"
    );

    let after = db
        .search_players_keyset(&search, 100, last.next_cursor.as_ref())
        .expect("the page after the set should read");
    assert!(after.players.is_empty());
    assert!(after.next_cursor.is_none());
    assert_eq!(after.total, None);
}

/// Given a deep position in a two-thousand-player world
/// When the page there is read by keyset and by `OFFSET`
/// Then both hold the same players, so a deep seek is not a scan with extra steps.
#[test]
fn a_deep_keyset_page_matches_the_offset_page_at_the_same_position() {
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&synthetic_snapshot(4, 2_000))
        .expect("the world should import");
    let search = PlayerSearch::default();

    let anchor = db
        .search_players_page(&search, Page::new(1_985, 5))
        .expect("the anchor page should read");
    let cursor = PlayerKeysetCursor::after(
        search.sort,
        anchor.players.last().expect("a full anchor page"),
    );

    let deep_keyset = db
        .search_players_keyset(&search, 10, Some(&cursor))
        .expect("the deep keyset page should read");
    let deep_offset = db
        .search_players_page(&search, Page::new(1_990, 10))
        .expect("the deep offset page should read");

    assert_eq!(
        deep_keyset.total, None,
        "the first page carried the total; a deep seek does not count the world again"
    );
    assert_eq!(deep_offset.total, 2_000);
    assert_eq!(ids(&deep_keyset.players), ids(&deep_offset.players));
    assert_eq!(deep_keyset.players.len(), 10);
}

/// Given a keyset cursor taken in one order
/// When it is used against a different order, or carries no player id
/// Then the query refuses it instead of skipping or repeating rows.
#[test]
fn a_cursor_from_another_order_or_without_an_id_is_refused() {
    let mut db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    db.import(&tied_and_null_snapshot())
        .expect("the tied world should import");
    let by_overall = PlayerSearch::default();
    let page = db
        .search_players_keyset(&by_overall, 5, None)
        .expect("the first page should read");
    let cursor = page.next_cursor.expect("a full page has a cursor");

    let by_name = PlayerSearch {
        sort: PlayerSort::ascending(PlayerSortColumn::Name),
        ..PlayerSearch::default()
    };
    assert!(
        db.search_players_keyset(&by_name, 5, Some(&cursor))
            .is_err(),
        "a cursor read in another direction is not a position"
    );

    let mut blank = cursor.clone();
    blank.player_id = String::new();
    assert!(
        db.search_players_keyset(&by_overall, 5, Some(&blank))
            .is_err(),
        "a cursor with no id cannot page stably"
    );

    let mut mismatched = cursor;
    mismatched.sort_value = PlayerSortValue::Text(String::from("name 1"));
    assert!(
        db.search_players_keyset(&by_overall, 5, Some(&mismatched))
            .is_err(),
        "a text value cannot page a numeric order"
    );
}

/// Given a keyset cursor at a rated row and one inside the NULL group, for each numeric order
/// When SQLite explains every statement the walk would run
/// Then every branch is an index seek with no temp B-tree and no full table scan.
#[test]
fn keyset_page_statements_are_index_seeks() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let rated = summary_row("p-050", "Name 1", Some(52), Some(1992));
    let unrated = summary_row("p-050", "Name 1", None, None);

    let mut probes: Vec<(PlayerSort, PlayerKeysetCursor)> = Vec::new();
    for sort in every_player_order() {
        probes.push((sort, PlayerKeysetCursor::after(sort, &rated)));
        if sort.column != PlayerSortColumn::Name {
            probes.push((sort, PlayerKeysetCursor::after(sort, &unrated)));
        }
    }

    for (sort, cursor) in probes {
        let search = PlayerSearch {
            sort,
            ..PlayerSearch::default()
        };
        for (sql, params) in
            crate::world_db::queries::test_support::keyset_sql(&search, Some(&cursor), 10)
        {
            let plan = query_plan(&db, &sql, &params);
            assert!(
                !plan.iter().any(|line| line.contains("USE TEMP B-TREE")),
                "{sort:?} sorted a keyset page: {plan:#?}"
            );
            assert_no_full_table_scan(&plan, "p");
            assert_no_full_table_scan(&plan, "c");
            assert!(
                plan.iter().any(|line| line.contains("SEARCH p")),
                "{sort:?} must seek, not walk, the players index: {plan:#?}"
            );
        }

        let (count_sql, count_params) = crate::world_db::queries::test_support::count_sql(&search);
        let count_plan = query_plan(&db, &count_sql, &count_params);
        assert_no_full_table_scan(&count_plan, "p");
        assert!(
            !count_plan
                .iter()
                .any(|line| line.contains("USE TEMP B-TREE"))
        );
    }
}

/// A player row, for building a keyset position without reading the database.
fn summary_row(
    id: &str,
    full_name: &str,
    overall: Option<u8>,
    birth_year: Option<i32>,
) -> PlayerSummary {
    PlayerSummary {
        id: String::from(id),
        full_name: String::from(full_name),
        position: String::from("Striker"),
        position_group: String::from("forward"),
        nationality: String::from("ENG"),
        birth_date: birth_year.map(|year| format!("{year}-01-01")),
        birth_year,
        current_club_id: None,
        current_club_name: None,
        overall,
        potential: overall,
    }
}

/// Given the first keyset page
/// When SQLite explains its statement
/// Then it too is an index walk with no temp sort, like every later page.
#[test]
fn the_first_keyset_page_is_index_backed_in_every_order() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    for sort in every_player_order() {
        let search = PlayerSearch {
            sort,
            ..PlayerSearch::default()
        };
        let statements = crate::world_db::queries::test_support::keyset_sql(&search, None, 10);
        assert_eq!(statements.len(), 1, "the first page is one statement");
        for (sql, params) in statements {
            let plan = query_plan(&db, &sql, &params);
            assert!(
                !plan.iter().any(|line| line.contains("USE TEMP B-TREE")),
                "{sort:?}: {plan:#?}"
            );
            assert_no_full_table_scan(&plan, "p");
        }
    }
}

// ---------------------------------------------------------------------------
// M2: the sync store
// ---------------------------------------------------------------------------

fn fetch_entry(provider: &str, endpoint: &str, seconds: i64) -> NewFetchEntry {
    NewFetchEntry {
        provider: String::from(provider),
        endpoint: String::from(endpoint),
        status: Some(200),
        fetched_at: at(seconds),
        payload_hash: Some(format!("hash-{seconds}")),
    }
}

fn canonical_link(provider_id: &str, entity_id: &str, confidence: u8) -> CanonicalLink {
    CanonicalLink {
        provider: String::from("api-football"),
        provider_entity_id: String::from(provider_id),
        entity_kind: String::from("player"),
        entity_id: String::from(entity_id),
        confidence,
        as_of: Some(String::from("2026-11-20")),
    }
}

/// Given a log with three requests for one route
/// When its newest entry is read, its entries are listed and an old cutoff is pruned
/// Then the newest is the last one written, the list is oldest first, and pruned rows are gone.
#[test]
fn the_request_log_reads_newest_first_and_prunes_what_it_no_longer_needs() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let sync = db.sync();
    for seconds in [1_000_000, 2_000_000, 3_000_000] {
        sync.record_fetch(&fetch_entry("api-football", "/fixtures", seconds))
            .expect("a request should be recorded");
    }
    sync.record_fetch(&fetch_entry("local-fixture", "/players", 1_500_000))
        .expect("another provider's request should be recorded");

    let newest = sync
        .latest_fetch("api-football", "/fixtures")
        .expect("the log should read")
        .expect("the route was fetched");
    assert_eq!(newest.fetched_at, at(3_000_000));
    assert_eq!(newest.status, Some(200));
    assert_eq!(newest.payload_hash.as_deref(), Some("hash-3000000"));
    assert_eq!(newest.provider, "api-football");

    let entries = sync
        .fetch_entries("api-football")
        .expect("the log should list");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.fetched_at)
            .collect::<Vec<_>>(),
        vec![at(1_000_000), at(2_000_000), at(3_000_000)]
    );

    let cutoff = prune_cutoff(at(10_000_000), chrono::Duration::days(30));
    let pruned = sync
        .prune_fetch_log_before(cutoff)
        .expect("the prune should run");
    assert_eq!(pruned, 4, "every row is older than the window");
    assert_eq!(
        sync.fetch_entries("api-football")
            .expect("the log should list")
            .len(),
        0
    );

    sync.record_fetch(&fetch_entry("api-football", "/fixtures", 9_999_000))
        .expect("a fresh request should be recorded");
    assert_eq!(
        sync.prune_fetch_log_before(cutoff)
            .expect("the prune should run"),
        0,
        "an entry inside the window is kept"
    );

    assert!(
        sync.record_fetch(&NewFetchEntry {
            provider: String::from("   "),
            ..fetch_entry("api-football", "/fixtures", 1_000)
        })
        .is_err(),
        "a blank provider is refused before it reaches SQLite"
    );
}

/// Given a provider identity already linked, the same link again, and a second canonical id
/// When each is upserted
/// Then they are inserted, updated and refused as a conflict without writing the second id.
#[test]
fn a_provider_identity_upserts_and_refuses_a_second_canonical_id() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let sync = db.sync();

    assert_eq!(
        sync.upsert_entity_link(&canonical_link("874", "player-1", 90), Some("world-sync"))
            .expect("the first link should write"),
        LinkOutcome::Inserted
    );
    assert_eq!(
        sync.upsert_entity_link(&canonical_link("874", "player-1", 95), Some("world-sync"))
            .expect("the same link should update"),
        LinkOutcome::Updated
    );

    let stored = sync
        .canonical_link("api-football", "874", "player")
        .expect("the link should read")
        .expect("the link exists");
    assert_eq!(stored.entity_id, "player-1");
    assert_eq!(stored.confidence, 95);
    assert_eq!(stored.as_of.as_deref(), Some("2026-11-20"));

    let raw: f64 = db
        .conn()
        .query_row("SELECT confidence FROM entity_links", [], |row| row.get(0))
        .expect("the raw confidence should read");
    assert!(
        (raw - 0.95).abs() < f64::EPSILON,
        "the table's unit is 0.0..=1.0: {raw}"
    );

    assert_eq!(
        sync.upsert_entity_link(&canonical_link("874", "player-2", 80), None)
            .expect("a conflict is not an error"),
        LinkOutcome::Conflict {
            existing_entity_id: String::from("player-1")
        }
    );
    assert_eq!(
        sync.entity_ids_for("api-football", "874", "player")
            .expect("the ids should read"),
        vec![String::from("player-1")],
        "a conflict writes nothing"
    );
    assert_eq!(
        sync.entity_ids_for(" api-football ", " 874 ", " player ")
            .expect("padded lookup should trim and read"),
        vec![String::from("player-1")],
        "trimming in entity_ids_for matches canonical_link"
    );
    assert_eq!(
        sync.resolve_entity_link("api-football", "874", "player")
            .expect("the link should resolve"),
        Some(String::from("player-1"))
    );
    assert_eq!(
        sync.resolve_entity_link("api-football", "875", "player")
            .expect("an unlinked identity resolves to nothing"),
        None
    );
}

/// Given a cross-walk an older import left with two canonical ids for one identity
/// When the conflicts are listed
/// Then the identity appears once with both ids, sorted, and it resolves to neither.
#[test]
fn a_cross_walk_with_two_canonical_ids_is_reported_and_does_not_resolve() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let sync = db.sync();
    for entity_id in ["player-2", "player-1"] {
        db.conn()
            .execute(
                "INSERT INTO entity_links
                     (provider, provider_id, entity_kind, entity_id, confidence, reviewer, as_of)
                 VALUES ('api-football', '874', 'player', ?1, 0.5, 'importer', '2026-11-19')",
                [entity_id],
            )
            .expect("the legacy link should insert");
    }

    let conflicts = sync
        .conflicting_entity_links()
        .expect("the cross-walk should be readable");
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicts[0].entity_ids,
        vec![String::from("player-1"), String::from("player-2")]
    );
    assert_eq!(conflicts[0].provider_id, "874");
    assert_eq!(
        sync.entity_ids_for(" api-football ", " 874 ", " player ")
            .expect("padded lookup should trim and read"),
        vec![String::from("player-1"), String::from("player-2")],
        "trimming in entity_ids_for matches canonical_link"
    );
    assert_eq!(
        sync.resolve_entity_link("api-football", "874", "player")
            .expect("an ambiguous identity resolves to nothing"),
        None
    );
    assert_eq!(
        sync.canonical_link("api-football", "874", "player")
            .expect("an ambiguous identity reads as nothing"),
        None,
        "two canonical ids are a decision, not a first row"
    );
}

/// Given two runs of one route, the newer with a validator and an expiry
/// When the newest run and the freshness policy input are read
/// Then they are the newer run's, and the policy skips a route still inside its window.
#[test]
fn a_sync_run_stores_its_validator_and_feeds_the_freshness_policy() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let sync = db.sync();
    let run = |run_id: &str, started: i64, etag: &str| SyncRun {
        run_id: String::from(run_id),
        provider: String::from("api-football"),
        route: String::from("fixtures"),
        mode: String::from(SyncMode::Live.code()),
        status: String::from("ok"),
        etag: Some(String::from(etag)),
        expires_at: Some(at(started + 3_600)),
        started_at: at(started),
        finished_at: Some(at(started + 1)),
        records_upserted: 42,
        note: None,
    };

    sync.record_sync_run(&run("run-1", 1_000, "\"v1\""))
        .expect("the first run should record");
    sync.record_sync_run(&run("run-2", 5_000, "\"v2\""))
        .expect("the second run should record");
    sync.record_sync_run(&run("run-2", 5_000, "\"v2\""))
        .expect("recording the same run twice is idempotent");
    // Dry, failed, or in-flight runs must never overwrite live route freshness.
    sync.record_sync_run(&SyncRun {
        mode: String::from(SyncMode::DryRun.code()),
        ..run("run-dry", 5_500, "\"dry\"")
    })
    .expect("dry run should record");
    sync.record_sync_run(&SyncRun {
        status: String::from("failed"),
        etag: None,
        expires_at: None,
        ..run("run-failed", 5_600, "")
    })
    .expect("failed run should record");
    sync.record_sync_run(&SyncRun {
        status: String::from("planned"),
        etag: None,
        expires_at: None,
        ..run("run-planned", 5_700, "")
    })
    .expect("planned run should record");

    let newest = sync
        .latest_sync_run("api-football", "fixtures")
        .expect("the ledger should read")
        .expect("the route ran");
    assert_eq!(newest.run_id, "run-planned");

    let freshness = sync
        .route_freshness("api-football", "fixtures")
        .expect("the policy input should read")
        .expect("the route ran live");
    assert_eq!(freshness.etag.as_deref(), Some("\"v2\""));
    assert_eq!(freshness.expires_at, Some(at(8_600)));
    assert_eq!(
        plan_request(at(6_000), Some(&freshness)),
        SyncRequestPlan::SkipFresh
    );
    assert_eq!(
        plan_request(at(9_000), Some(&freshness)),
        SyncRequestPlan::FetchConditional {
            if_none_match: String::from("\"v2\"")
        }
    );

    assert_eq!(
        sync.sync_run("run-1")
            .expect("a run should read by id")
            .expect("the run exists")
            .etag
            .as_deref(),
        Some("\"v1\"")
    );
    assert!(
        sync.latest_sync_run("api-football", "players")
            .expect("an unrun route reads")
            .is_none()
    );
    assert!(
        sync.record_sync_run(&SyncRun {
            run_id: String::from(" "),
            ..run("run-3", 6_000, "\"v3\"")
        })
        .is_err(),
        "a blank run id never reaches SQLite"
    );
}

/// Given a competition and two of its clubs for one season
/// When they are upserted twice and read back
/// Then the row round trips with its NULLs intact and membership is idempotent both ways.
#[test]
fn competitions_and_their_club_membership_round_trip_idempotently() {
    let db = WorldDatabase::open_in_memory().expect("in-memory world should open");
    let sync = db.sync();
    let competition = CompetitionRow {
        id: String::from("comp-1"),
        name: String::from("Premier League"),
        short_name: String::from("EPL"),
        country: Some(String::from("ENG")),
        tier: 1,
        kind: String::from("league"),
        confederation: None,
        updated_at: at(1_000),
    };
    for _ in 0..2 {
        sync.upsert_competition(&competition)
            .expect("a competition should upsert");
        for club_id in ["club-001", "club-000"] {
            sync.upsert_competition_club(&CompetitionClubRow {
                competition_id: String::from("comp-1"),
                club_id: String::from(club_id),
                season_year: 2026,
            })
            .expect("membership should upsert");
        }
    }

    let stored = sync
        .competition("comp-1")
        .expect("the competition should read")
        .expect("the competition exists");
    assert_eq!(stored, competition);
    assert_eq!(
        stored.confederation, None,
        "an unknown confederation stays NULL"
    );
    assert_eq!(count_rows(&db, "world_competitions"), 1);

    assert_eq!(
        sync.competition_clubs("comp-1", 2026)
            .expect("the field should read"),
        vec![String::from("club-000"), String::from("club-001")]
    );
    assert_eq!(
        sync.competitions_for_club("club-001", 2026)
            .expect("a club's competitions should read"),
        vec![String::from("comp-1")]
    );
    assert!(
        sync.competitions_for_club("club-001", 2025)
            .expect("another season reads")
            .is_empty()
    );
    assert_eq!(count_rows(&db, "world_competition_clubs"), 2);

    assert!(
        sync.upsert_competition(&CompetitionRow {
            id: String::from("  "),
            ..competition
        })
        .is_err(),
        "a blank competition id is refused"
    );
}
