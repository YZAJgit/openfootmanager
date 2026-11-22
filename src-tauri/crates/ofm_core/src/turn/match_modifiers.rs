//! The small lifts a match takes from outside the engine's own model, folded into each player's
//! `morale_factor` before kick-off: his character on the occasion (`character`), the eleven's
//! on-pitch cohesion and the side's familiarity with its shape (`chemistry`). Together they move a
//! player by at most 4% either way, so no one of them decides a match.

use std::collections::HashMap;

use domain::character::PairChemistry;
use domain::league::Fixture;
use domain::player::Player;
use domain::team::Team;
use engine::{PlayerData, TeamData};

use crate::character::{assessed_character, profile_match_factor};
use crate::chemistry::{familiarity_match_factor, team_cohesion_factor};

/// The most the modifiers together move a player's play either way.
const MODIFIER_RANGE: f64 = 0.04;
/// A side that has won none of its last this many and lost at least `PRESSURE_LOSSES` of them is
/// under pressure.
const PRESSURE_FORM: usize = 3;
const PRESSURE_LOSSES: usize = 2;

/// What the match means to a side.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MatchOccasion {
    /// A tie played for something other than league points: a cup or continental match.
    pub is_big_match: bool,
    /// The side comes in on a run of defeats.
    pub is_under_pressure: bool,
}

/// What `fixture` means to `team`.
pub(crate) fn occasion_for(fixture: &Fixture, team: Option<&Team>) -> MatchOccasion {
    let recent = team.map_or(&[][..], |team| {
        let from = team.form.len().saturating_sub(PRESSURE_FORM);
        &team.form[from..]
    });
    let losses = recent.iter().filter(|result| *result == "L").count();
    let wins = recent.iter().filter(|result| *result == "W").count();
    MatchOccasion {
        is_big_match: fixture.is_competitive() && !fixture.counts_for_league_standings(),
        is_under_pressure: wins == 0 && losses >= PRESSURE_LOSSES,
    }
}

/// Fold the modifiers into the morale factor of every player of `side` and `bench`, and return
/// each player's modifier, so a later refit of his morale (a team talk) keeps it. `club` holds the
/// side's players, to read their character and nationality from; `pairs` is the world's pair
/// chemistry and `seed` its seed, which assesses a character nobody has yet.
pub(crate) fn fold_match_modifiers(
    (pairs, seed): (&[PairChemistry], u64),
    team: Option<&Team>,
    occasion: MatchOccasion,
    (side, bench): (&mut TeamData, &mut [PlayerData]),
    club: &[&Player],
) -> HashMap<String, f64> {
    let by_id: HashMap<&str, &Player> = club.iter().map(|p| (p.id.as_str(), *p)).collect();
    let eleven: Vec<&Player> = side
        .players
        .iter()
        .filter_map(|p| by_id.get(p.id.as_str()).copied())
        .collect();
    let familiarity = team.map_or(1.0, |team| {
        familiarity_match_factor(&team.tactical_familiarity, &side.formation)
    });
    let starters = team_cohesion_factor(pairs, &eleven) * familiarity;
    let mut modifiers = HashMap::new();
    let mut fold = |player: &mut PlayerData, side_factor: f64| {
        let character = by_id.get(player.id.as_str()).map_or(1.0, |p| {
            let profile = assessed_character(p, seed);
            profile_match_factor(&profile, occasion.is_big_match, occasion.is_under_pressure)
        });
        let modifier = (side_factor * character).clamp(1.0 - MODIFIER_RANGE, 1.0 + MODIFIER_RANGE);
        player.morale_factor *= modifier;
        modifiers.insert(player.id.clone(), modifier);
    };
    for player in &mut side.players {
        fold(player, starters);
    }
    // A substitute joins a side that already knows its shape, but not the partnerships on it.
    for player in bench.iter_mut() {
        fold(player, familiarity);
    }
    modifiers
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::character::{CharacterProfile, TeamTacticalFamiliarity};
    use domain::league::FixtureCompetition;

    fn fixture(competition: FixtureCompetition) -> Fixture {
        Fixture {
            competition,
            ..Fixture::default()
        }
    }

    fn team_with_form(form: &[&str]) -> Team {
        let mut team = Team::new(
            "h".into(),
            "H".into(),
            "H".into(),
            "ENG".into(),
            "City".into(),
            "Ground".into(),
            10_000,
        );
        team.form = form.iter().map(|r| r.to_string()).collect();
        team
    }

    /// Given a league match, a cup tie, and sides on a winning run, a losing run and a mixed one
    /// When the occasion is read
    /// Then only the cup tie is big, and only the side that has lost two of three without a win
    ///      is under pressure
    #[test]
    fn a_cup_tie_is_big_and_a_losing_run_is_pressure() {
        let league = fixture(FixtureCompetition::League);
        let cup = fixture(FixtureCompetition::Cup);
        let losing = team_with_form(&["W", "D", "L", "L"]);
        let mixed = team_with_form(&["L", "W", "L"]);
        assert!(!occasion_for(&league, None).is_big_match);
        assert!(occasion_for(&cup, None).is_big_match);
        assert!(occasion_for(&league, Some(&losing)).is_under_pressure);
        assert!(!occasion_for(&league, Some(&mixed)).is_under_pressure);
        assert!(!occasion_for(&league, None).is_under_pressure);
    }

    fn player(id: &str) -> Player {
        let mut p = Player::new(
            id.into(),
            id.into(),
            id.into(),
            "1995-01-01".into(),
            "ENG".into(),
            domain::player::Position::CentralMidfielder,
            crate::test_support::uniform_attributes(60),
        );
        p.character = CharacterProfile {
            big_matches: 99,
            pressure_handling: 99,
            ..CharacterProfile::default()
        };
        p
    }

    fn engine_player(player: &Player) -> PlayerData {
        let mut data =
            super::super::squad::to_engine_player(player, engine::PlayerRole::Standard, None);
        data.morale_factor = 1.0;
        data
    }

    /// Given an eleven of two bonded big-game players in a shape they know fully, in a cup tie
    /// while under pressure, and a bench of one
    /// When the modifiers are folded
    /// Then every factor is lifted, none past 1.04, and each modifier is returned for a refit
    #[test]
    fn the_modifiers_lift_a_bonded_side_on_its_occasion_by_at_most_four_percent() {
        let squad = [player("a"), player("b"), player("c")];
        let club: Vec<&Player> = squad.iter().collect();
        let pairs = vec![PairChemistry::new("a", "b", 900, 100)];
        let mut team = team_with_form(&[]);
        team.tactical_familiarity = TeamTacticalFamiliarity::new("4-4-2", 100.0, 30);
        let mut side = TeamData {
            id: "h".into(),
            name: "H".into(),
            formation: "4-4-2".into(),
            play_style: engine::PlayStyle::Balanced,
            players: vec![engine_player(&squad[0]), engine_player(&squad[1])],
            tactics: engine::TacticsConfig::default(),
        };
        let mut bench = vec![engine_player(&squad[2])];
        let occasion = MatchOccasion {
            is_big_match: true,
            is_under_pressure: true,
        };

        let modifiers = fold_match_modifiers(
            (&pairs, 1),
            Some(&team),
            occasion,
            (&mut side, &mut bench),
            &club,
        );

        for player in side.players.iter().chain(&bench) {
            assert!(player.morale_factor > 1.0, "{}", player.id);
            assert!(player.morale_factor <= 1.04 + 1e-12, "{}", player.id);
            assert_eq!(modifiers.get(&player.id), Some(&player.morale_factor));
        }
    }
}
