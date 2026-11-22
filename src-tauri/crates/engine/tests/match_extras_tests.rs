//! What a side brings beyond its eleven and its tactics — its captain and vice-captain, its
//! free-kick and corner takers, its manager's team talk — changes the match in both engines, and
//! a side that sets none of it plays exactly as before.

use engine::live_match::LiveMatchState;
use engine::{
    EventType, GoalSource, MatchCommand, MatchConfig, MatchEvent, MatchReport, PlayerData,
    SetPieceTakers, ShotBodyPart, Side, TeamData, TeamExtras, TeamTalkTone, simulate_with_extras,
    simulate_with_rng,
};
use rand::SeedableRng;
use rand::rngs::StdRng;

mod common;
use common::equal_side;

/// Enough matches that a small effect shows in at least one of them.
const SEEDS: u64 = 60;

/// The match as a list of what happened, to compare two runs event for event.
fn fingerprint(events: &[MatchEvent]) -> Vec<String> {
    events.iter().map(|event| format!("{event:?}")).collect()
}

fn instant(home: &TeamData, home_extras: &TeamExtras, seed: u64) -> MatchReport {
    let mut rng = StdRng::seed_from_u64(seed);
    simulate_with_extras(
        home,
        &equal_side("a"),
        &MatchConfig::default(),
        home_extras,
        &TeamExtras::default(),
        &mut rng,
    )
}

fn live_state(home: &TeamData, bench: Vec<PlayerData>) -> LiveMatchState {
    LiveMatchState::new(
        home.clone(),
        equal_side("a"),
        MatchConfig::default(),
        bench,
        vec![],
        false,
    )
}

fn play_out(mut state: LiveMatchState, seed: u64) -> MatchReport {
    let mut rng = StdRng::seed_from_u64(seed);
    while !state.step_minute(&mut rng).is_finished {}
    state.into_report()
}

fn live(home: &TeamData, home_extras: &TeamExtras, seed: u64) -> MatchReport {
    let mut state = live_state(home, vec![]);
    state.apply_extras(Side::Home, home_extras);
    play_out(state, seed)
}

/// Whether the extras change the match, in the engine `run` plays, for some seed.
fn changes_some_match(
    run: fn(&TeamData, &TeamExtras, u64) -> MatchReport,
    home: &TeamData,
    extras: &TeamExtras,
) -> bool {
    (0..SEEDS).any(|seed| {
        fingerprint(&run(home, &TeamExtras::default(), seed).events)
            != fingerprint(&run(home, extras, seed).events)
    })
}

fn with_set_pieces(change: impl FnOnce(&mut SetPieceTakers)) -> TeamExtras {
    let mut extras = TeamExtras::default();
    change(&mut extras.set_pieces);
    extras
}

fn with_talk(talk: TeamTalkTone) -> TeamExtras {
    TeamExtras {
        talk,
        ..TeamExtras::default()
    }
}

fn player_mut<'a>(team: &'a mut TeamData, id: &str) -> &'a mut PlayerData {
    team.players
        .iter_mut()
        .find(|player| player.id == id)
        .expect("in the side")
}

/// The home side with a born leader at centre-back.
fn side_with_a_leader() -> TeamData {
    let mut home = equal_side("h");
    player_mut(&mut home, "h_1").leadership = 95;
    home
}

/// Given nothing set for either side,
/// When the instant match is played with extras and without,
/// Then it is the same match, event for event: the extras draw nothing they were not given.
#[test]
fn a_side_that_sets_nothing_plays_exactly_as_before_in_the_instant_engine() {
    let home = equal_side("h");
    for seed in 0..SEEDS {
        let mut rng = StdRng::seed_from_u64(seed);
        let plain = simulate_with_rng(&home, &equal_side("a"), &MatchConfig::default(), &mut rng);
        let with_extras = instant(&home, &TeamExtras::default(), seed);
        assert_eq!(fingerprint(&plain.events), fingerprint(&with_extras.events));
    }
}

/// Given nothing set for either side,
/// When the live match is played with the empty extras applied and without them,
/// Then it is the same match, event for event.
#[test]
fn a_side_that_sets_nothing_plays_exactly_as_before_in_the_live_engine() {
    let home = equal_side("h");
    for seed in 0..SEEDS {
        let plain = play_out(live_state(&home, vec![]), seed);
        let with_extras = live(&home, &TeamExtras::default(), seed);
        assert_eq!(fingerprint(&plain.events), fingerprint(&with_extras.events));
    }
}

/// Given a born leader named captain,
/// When the match is played under the same seeds in each engine,
/// Then some match goes differently from the same side with no captain.
#[test]
fn a_captain_changes_the_match_in_both_engines() {
    let home = side_with_a_leader();
    let extras = with_set_pieces(|takers| takers.captain = Some("h_1".to_string()));
    assert!(changes_some_match(instant, &home, &extras));
    assert!(changes_some_match(live, &home, &extras));
}

/// Given a captain and a vice-captain,
/// When the captain is sent off, and then the vice-captain is substituted,
/// Then the vice-captain leads while he is on the pitch, and then nobody does.
#[test]
fn the_vice_captain_leads_once_the_captain_has_gone() {
    let home = equal_side("h");
    let mut substitute = player_mut(&mut equal_side("h"), "h_2").clone();
    substitute.id = "h_sub".to_string();
    let mut state = live_state(&home, vec![substitute]);
    state
        .apply_command(MatchCommand::SetCaptain {
            side: Side::Home,
            player_id: "h_1".to_string(),
        })
        .expect("captain named");
    state
        .apply_command(MatchCommand::SetViceCaptain {
            side: Side::Home,
            player_id: "h_2".to_string(),
        })
        .expect("vice-captain named");
    assert_eq!(state.acting_captain_id(Side::Home), Some("h_1"));

    state.test_send_off("h_1");
    assert_eq!(state.acting_captain_id(Side::Home), Some("h_2"));

    state
        .apply_command(MatchCommand::Substitute {
            side: Side::Home,
            player_off_id: "h_2".to_string(),
            player_on_id: "h_sub".to_string(),
        })
        .expect("substitution made");
    assert_eq!(state.acting_captain_id(Side::Home), None);
}

/// Given a captain sent off before kick-off and a born leader as vice-captain,
/// When the live match is played under the same seeds,
/// Then naming the vice-captain changes some match: he takes over what the captain brought.
#[test]
fn a_vice_captain_takes_over_when_the_captain_is_sent_off() {
    let mut home = equal_side("h");
    player_mut(&mut home, "h_2").leadership = 95;
    let changed = (0..SEEDS).any(|seed| {
        let run = |vice: Option<&str>| {
            let mut state = live_state(&home, vec![]);
            state.apply_team_extras(
                Side::Home,
                SetPieceTakers {
                    captain: Some("h_1".to_string()),
                    vice_captain: vice.map(str::to_string),
                    ..SetPieceTakers::default()
                },
                TeamTalkTone::Neutral,
            );
            state.test_send_off("h_1");
            fingerprint(&play_out(state, seed).events)
        };
        run(None) != run(Some("h_2"))
    });
    assert!(changed);
}

/// Given a striker of the ball named free-kick taker,
/// When the match is played under the same seeds in each engine,
/// Then some match goes differently, and over the seeds some of his free kicks are struck at
/// goal and land on the shot map as set pieces.
#[test]
fn a_free_kick_taker_who_shoots_changes_the_match_and_strikes_at_goal() {
    let mut home = equal_side("h");
    player_mut(&mut home, "h_9").shooting = 90;
    let extras = with_set_pieces(|takers| takers.free_kick_taker = Some("h_9".to_string()));
    assert!(changes_some_match(instant, &home, &extras));
    assert!(changes_some_match(live, &home, &extras));

    let struck = (0..SEEDS).any(|seed| {
        instant(&home, &extras, seed)
            .shot_map()
            .iter()
            .any(|shot| shot.body_part == ShotBodyPart::SetPiece && shot.shooter_id == "h_9")
    });
    assert!(struck, "no direct free kick in {SEEDS} matches");
}

/// Given a passer named free-kick taker,
/// When the match is played under the same seeds in each engine,
/// Then some match goes differently: he puts his free kicks into the box.
#[test]
fn a_free_kick_taker_who_crosses_changes_the_match() {
    let mut home = equal_side("h");
    player_mut(&mut home, "h_5").passing = 90;
    let extras = with_set_pieces(|takers| takers.free_kick_taker = Some("h_5".to_string()));
    assert!(changes_some_match(instant, &home, &extras));
    assert!(changes_some_match(live, &home, &extras));
}

/// Given a set-piece specialist with fine delivery named corner taker,
/// When the match is played under the same seeds in each engine,
/// Then some match goes differently from the same side with nobody named.
#[test]
fn a_corner_taker_changes_the_match_in_both_engines() {
    let mut home = equal_side("h");
    let taker = player_mut(&mut home, "h_6");
    taker.passing = 90;
    taker.vision = 90;
    taker.traits.push("SetPieceSpecialist".to_string());
    let extras = with_set_pieces(|takers| takers.corner_taker = Some("h_6".to_string()));
    assert!(changes_some_match(instant, &home, &extras));
    assert!(changes_some_match(live, &home, &extras));
}

/// Given each team talk but the neutral one,
/// When the match is played under the same seeds in each engine,
/// Then some match goes differently from the same side told nothing.
#[test]
fn every_team_talk_changes_the_match_in_both_engines() {
    let home = equal_side("h");
    for tone in [
        TeamTalkTone::Encourage,
        TeamTalkTone::DemandMore,
        TeamTalkTone::Calm,
        TeamTalkTone::FireUp,
        TeamTalkTone::Praise,
    ] {
        let extras = with_talk(tone);
        assert!(
            changes_some_match(instant, &home, &extras),
            "{tone:?} instant"
        );
        assert!(changes_some_match(live, &home, &extras), "{tone:?} live");
    }
}

/// Given a team talk given from the touchline in the second half,
/// When the live match is played under the same seeds,
/// Then the first half is untouched and some match goes differently after it.
#[test]
fn a_talk_at_the_break_changes_only_what_follows() {
    let home = equal_side("h");
    let changed = (0..SEEDS).any(|seed| {
        let run = |tone: TeamTalkTone| {
            let mut state = live_state(&home, vec![]);
            let mut rng = StdRng::seed_from_u64(seed);
            let mut first_half = Vec::new();
            while state.minute() < 45 {
                first_half.extend(state.step_minute(&mut rng).events);
            }
            state
                .apply_command(MatchCommand::ApplyTeamTalk {
                    side: Side::Home,
                    tone,
                })
                .expect("talk given");
            while !state.step_minute(&mut rng).is_finished {}
            (
                fingerprint(&first_half),
                fingerprint(&state.into_report().events),
            )
        };
        let (plain_first, plain_all) = run(TeamTalkTone::Neutral);
        let (fired_first, fired_all) = run(TeamTalkTone::FireUp);
        assert_eq!(
            plain_first, fired_first,
            "seed {seed}: the first half changed"
        );
        plain_all != fired_all
    });
    assert!(changed);
}

/// Given a whole match in each engine,
/// When its shot map is read,
/// Then every shot event is on it once, each side's xG is the sum of its shots' values, and
/// every value is within 0.01 and 0.95.
#[test]
fn every_shot_of_the_match_is_on_the_map_in_both_engines() {
    let home = equal_side("h");
    let mut total_shots = 0;
    for seed in 0..20 {
        for report in [
            instant(&home, &TeamExtras::default(), seed),
            live(&home, &TeamExtras::default(), seed),
        ] {
            let map = report.shot_map();
            for side in [Side::Home, Side::Away] {
                let open_play_events = report
                    .events
                    .iter()
                    .filter(|event| event.side == side)
                    .filter(|event| {
                        matches!(
                            event.event_type,
                            EventType::Goal
                                | EventType::ShotSaved
                                | EventType::ShotOffTarget
                                | EventType::ShotBlocked
                        )
                    })
                    .count();
                let penalty_events = report
                    .events
                    .iter()
                    .filter(|event| event.side == side)
                    .filter(|event| {
                        matches!(
                            event.event_type,
                            EventType::PenaltyGoal | EventType::PenaltyMiss
                        )
                    })
                    .count();
                let shots: Vec<_> = map.iter().filter(|shot| shot.side == side).collect();
                let penalties = shots
                    .iter()
                    .filter(|shot| shot.body_part == ShotBodyPart::Penalty)
                    .count();
                assert_eq!(shots.len() - penalties, open_play_events, "seed {seed}");
                assert_eq!(penalties, penalty_events, "seed {seed}");
                let sum: f32 = shots.iter().map(|shot| shot.xg).sum();
                let xg = match side {
                    Side::Home => report.home_xg(),
                    Side::Away => report.away_xg(),
                };
                assert!((sum - xg).abs() < 1e-4);
                assert!(shots.iter().all(|shot| (0.01..=0.95).contains(&shot.xg)));
                total_shots += shots.len();
            }
        }
    }
    assert!(total_shots > 0);
}

/// Given knockout ties played to a shootout,
/// When their shot maps are read,
/// Then no shootout kick is on them.
#[test]
fn shootout_kicks_are_not_shots_of_the_match() {
    let mut shootouts = 0;
    for seed in 0..200 {
        let mut state = LiveMatchState::new(
            equal_side("h"),
            equal_side("a"),
            MatchConfig::default(),
            vec![],
            vec![],
            true,
        );
        let mut rng = StdRng::seed_from_u64(seed);
        while !state.step_minute(&mut rng).is_finished {}
        let report = state.into_report();
        if report.home_penalties.is_none() {
            continue;
        }
        shootouts += 1;
        let in_match_penalties = report
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type,
                    EventType::PenaltyGoal | EventType::PenaltyMiss
                )
            })
            .count();
        let penalties_on_the_map = report
            .shot_map()
            .iter()
            .filter(|shot| shot.body_part == ShotBodyPart::Penalty)
            .count();
        assert_eq!(penalties_on_the_map, in_match_penalties, "seed {seed}");
        if shootouts >= 3 {
            return;
        }
    }
    assert!(shootouts > 0, "no shootout in 200 knockout ties");
}

/// Given a free-kick taker who shoots, over many matches,
/// When the goals are read off the reports,
/// Then some of his are scored straight from the free kick.
#[test]
fn a_direct_free_kick_goal_is_a_free_kick_goal() {
    let mut home = equal_side("h");
    player_mut(&mut home, "h_9").shooting = 95;
    let extras = with_set_pieces(|takers| takers.free_kick_taker = Some("h_9".to_string()));
    let scored = (0..400).any(|seed| {
        instant(&home, &extras, seed)
            .goals
            .iter()
            .any(|goal| goal.scorer_id == "h_9" && goal.goal_source == GoalSource::FreeKick)
    });
    assert!(scored);
}
