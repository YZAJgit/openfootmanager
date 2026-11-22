//! Every role is wired into both engines: each one a player may be given in a line changes the
//! match for him there, through the phases that line is consulted on.

use engine::live_match::LiveMatchState;
use engine::{
    MatchConfig, MatchEvent, PlayerRole, Position, TeamData, role_has_match_effect,
    simulate_with_rng,
};
use rand::SeedableRng;
use rand::rngs::StdRng;

mod common;
use common::equal_side;

/// Enough matches that the gentlest role (a keeper's, on his side's few build-ups from a goal
/// kick) shows in one of them; the search stops at the first match that differs.
const SEEDS: u64 = 300;

/// An equal side with every player of `line` on `role`.
fn side_with(line: Position, role: PlayerRole) -> TeamData {
    let mut home = equal_side("h");
    for player in home.players.iter_mut().filter(|p| p.position == line) {
        player.role = role;
    }
    home
}

fn fingerprint(events: &[MatchEvent]) -> Vec<String> {
    events.iter().map(|event| format!("{event:?}")).collect()
}

fn instant(home: &TeamData, seed: u64) -> Vec<String> {
    let mut rng = StdRng::seed_from_u64(seed);
    fingerprint(
        &simulate_with_rng(home, &equal_side("a"), &MatchConfig::default(), &mut rng).events,
    )
}

fn live(home: &TeamData, seed: u64) -> Vec<String> {
    let mut state = LiveMatchState::new(
        home.clone(),
        equal_side("a"),
        MatchConfig::default(),
        vec![],
        vec![],
        false,
    );
    let mut rng = StdRng::seed_from_u64(seed);
    let mut events = Vec::new();
    loop {
        let minute = state.step_minute(&mut rng);
        events.extend(minute.events);
        if minute.is_finished {
            return fingerprint(&events);
        }
    }
}

/// Given every role and every line it may be played in,
/// When asked through the engine's public answer,
/// Then every one has a match effect.
#[test]
fn every_role_has_a_match_effect_in_every_line_it_suits() {
    for role in PlayerRole::ALL {
        for &line in role.position_groups() {
            assert!(role_has_match_effect(role, line), "{role:?} as {line:?}");
        }
    }
}

/// Given each role on every player of a line it suits,
/// When the match is played under the same seeds in each engine,
/// Then some match goes differently from the same side on Standard: the role reaches play.
#[test]
fn every_role_changes_some_match_in_both_engines() {
    for role in PlayerRole::ALL {
        for &line in role.position_groups() {
            let standard = side_with(line, PlayerRole::Standard);
            let changed = side_with(line, role);
            for (name, run) in [
                ("instant", instant as fn(&TeamData, u64) -> Vec<String>),
                ("live", live),
            ] {
                let differs = (0..SEEDS).any(|seed| run(&standard, seed) != run(&changed, seed));
                assert!(differs, "{role:?} as {line:?} changed nothing in {name}");
            }
        }
    }
}
