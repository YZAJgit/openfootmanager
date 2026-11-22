//! On-pitch understanding: how well two team-mates read each other's game from the minutes they
//! have played together and the goals they have made together, how well a side knows its shape,
//! and what both do to a match.
//!
//! This is a football model of an on-pitch partnership, built only from match data. It is never a
//! statement about two real people's personal relationship, and nothing here should be presented
//! as one.

use std::collections::{HashMap, HashSet};

use domain::character::{FAMILIARITY_MAX, PairChemistry, TeamTacticalFamiliarity};
use domain::player::Player;
use domain::team::Team;

/// The most cohesion moves a side's play either way.
const COHESION_RANGE: f64 = 0.04;
/// What a side's mean pair bond (`-100..=100`) can move its play.
const BOND_WEIGHT: f64 = 0.025;
/// What sharing a football nation, or being of an age, can move it: centred so a side of half
/// such pairs reads neutral, and grown in with the minutes the eleven has played together
/// (`TOGETHER_MINUTES`), so a side that has not yet played together reads exactly neutral.
const NATION_WEIGHT: f64 = 0.01;
const AGE_GROUP_WEIGHT: f64 = 0.01;
/// The mean minutes a pair of the eleven must have shared for nation and age to count in full.
const TOGETHER_MINUTES: f64 = 900.0;
/// Two players born within this many years of each other are of an age.
const AGE_GROUP_YEARS: i32 = 4;

/// A full match together builds this much bond; less time together builds less.
const BOND_PER_FULL_MATCH: u32 = 2;
/// A goal one made for the other builds this much.
const BOND_PER_GOAL_TOGETHER: i16 = 3;
/// What the result does to the bond of every pair of a side: a win builds it, a defeat sets it
/// back by as much as a full match together builds, a heavy one (`HEAVY_DEFEAT` goals or more)
/// by more.
const BOND_FOR_A_WIN: i16 = 1;
const BOND_FOR_A_DEFEAT: i16 = -2;
const BOND_FOR_A_HEAVY_DEFEAT: i16 = -3;
const HEAVY_DEFEAT: u8 = 3;

/// The familiarity a side is taken to have with the shape it is first seen in: a side that has
/// played for a while knows its own shape, so tracking starts at the neutral middle.
const FIRST_SEEN_FAMILIARITY: f32 = 50.0;
/// Each match in the same shape closes this share of the gap to full familiarity.
const FAMILIARITY_GAIN: f32 = 0.08;
/// The share of its familiarity a side keeps when it changes shape.
const FAMILIARITY_KEPT_ON_CHANGE: f32 = 0.4;
/// The most familiarity moves a side's play either way.
const FAMILIARITY_RANGE: f64 = 0.02;

/// Add one match to the pairs' shared minutes and bonds. `lineups` holds, for each side, every
/// player who took part: the home side first, then the away side, whose results move their
/// pairs' bonds. `goal_assist_pairs` holds (scorer, assister) for each assisted goal. Pairs are
/// kept in canonical order, sorted, so each pair has exactly one entry.
pub fn record_match_chemistry(
    pairs: &mut Vec<PairChemistry>,
    report: &engine::MatchReport,
    lineups: &[Vec<String>],
    goal_assist_pairs: &[(String, String)],
) {
    let minutes = |id: &str| {
        report
            .player_stats
            .get(id)
            .map_or(0, |stats| u32::from(stats.minutes_played))
    };
    let mut changes: HashMap<(String, String), (u32, i16)> = HashMap::new();
    for (side, lineup) in lineups.iter().enumerate() {
        let result = result_bond(report, side);
        for (index, first) in lineup.iter().enumerate() {
            for second in &lineup[index + 1..] {
                // Two players' minutes overlap by at most the shorter of the two.
                let shared = minutes(first).min(minutes(second));
                if shared == 0 || first == second {
                    continue;
                }
                let bond = i16::try_from(shared * BOND_PER_FULL_MATCH / 90).unwrap_or(0) + result;
                let entry = changes.entry(canonical_key(first, second)).or_default();
                entry.0 += shared;
                entry.1 += bond;
            }
        }
    }
    for (scorer, assister) in goal_assist_pairs {
        if scorer != assister {
            changes
                .entry(canonical_key(scorer, assister))
                .or_default()
                .1 += BOND_PER_GOAL_TOGETHER;
        }
    }
    apply_changes(pairs, changes);
}

/// What the result does to the bonds of the side at `side` in the lineups (0 home, 1 away).
fn result_bond(report: &engine::MatchReport, side: usize) -> i16 {
    let (own, other) = match side {
        0 => (report.home_goals, report.away_goals),
        1 => (report.away_goals, report.home_goals),
        _ => return 0,
    };
    if own > other {
        BOND_FOR_A_WIN
    } else if other >= own.saturating_add(HEAVY_DEFEAT) {
        BOND_FOR_A_HEAVY_DEFEAT
    } else if other > own {
        BOND_FOR_A_DEFEAT
    } else {
        0
    }
}

/// The (scorer, assister) of each assisted goal in `report`.
pub fn goal_assist_pairs(report: &engine::MatchReport) -> Vec<(String, String)> {
    report
        .goals
        .iter()
        .filter_map(|goal| {
            goal.assist_id
                .as_ref()
                .map(|assister| (goal.scorer_id.clone(), assister.clone()))
        })
        .collect()
}

/// Drop every pair whose two players are no longer at the same club: understanding is built and
/// kept on the pitch together, so it goes when one of them leaves.
pub fn retain_team_mates(pairs: &mut Vec<PairChemistry>, players: &[Player]) {
    let club_of: HashMap<&str, &str> = players
        .iter()
        .filter_map(|p| p.team_id.as_deref().map(|team| (p.id.as_str(), team)))
        .collect();
    pairs.retain(|pair| {
        match (
            club_of.get(pair.player_a.as_str()),
            club_of.get(pair.player_b.as_str()),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    });
}

/// How well an eleven plays together: 0.96..=1.04, exactly 1.0 for a side that has not played
/// together. Built from the pairs' on-pitch bond, and a little from shared football nation (a
/// common football language) and age group, which count as the side plays together.
pub fn team_cohesion_factor(pairs: &[PairChemistry], xi_players: &[&Player]) -> f64 {
    let mut pair_count = 0u32;
    let (mut bond_total, mut minutes_total) = (0f64, 0f64);
    let (mut same_nation, mut same_age) = (0u32, 0u32);
    for (index, first) in xi_players.iter().enumerate() {
        for second in &xi_players[index + 1..] {
            pair_count += 1;
            if let Some(pair) = pair_between(pairs, &first.id, &second.id) {
                bond_total += f64::from(pair.bond);
                minutes_total += f64::from(pair.shared_minutes);
            }
            if first.football_nation == second.football_nation {
                same_nation += 1;
            }
            if of_an_age(first, second) {
                same_age += 1;
            }
        }
    }
    if pair_count == 0 {
        return 1.0;
    }
    let count = f64::from(pair_count);
    let bond = BOND_WEIGHT * bond_total / count / 100.0;
    let together = (minutes_total / count / TOGETHER_MINUTES).min(1.0);
    let nation = NATION_WEIGHT * (f64::from(same_nation) / count - 0.5) * together;
    let age = AGE_GROUP_WEIGHT * (f64::from(same_age) / count - 0.5) * together;
    (1.0 + bond + nation + age).clamp(1.0 - COHESION_RANGE, 1.0 + COHESION_RANGE)
}

/// What a side's familiarity with its shape does to its play: 0.98..=1.02, 1.0 at the neutral
/// middle, and 1.0 for a side not yet seen in any shape. A side lining up in a shape other than
/// the one it knows plays as one that has just changed shape.
pub fn familiarity_match_factor(familiarity: &TeamTacticalFamiliarity, formation: &str) -> f64 {
    if familiarity.formation.is_empty() {
        return 1.0;
    }
    let known = if familiarity.formation == formation {
        familiarity.familiarity
    } else {
        familiarity.familiarity * FAMILIARITY_KEPT_ON_CHANGE
    };
    let centred = f64::from(TeamTacticalFamiliarity::hold(known)) / f64::from(FAMILIARITY_MAX);
    1.0 + FAMILIARITY_RANGE * (2.0 * centred - 1.0)
}

/// Add one match in the side's current formation to its familiarity: a side first seen starts
/// at the middle, one in the same shape grows toward full, and one that has changed shape keeps
/// part of what it knew.
pub fn update_tactical_familiarity(team: &mut Team) {
    let current = &mut team.tactical_familiarity;
    if current.formation.is_empty() {
        *current = TeamTacticalFamiliarity::new(team.formation.clone(), FIRST_SEEN_FAMILIARITY, 1);
    } else if current.formation == team.formation {
        let grown =
            current.familiarity + (FAMILIARITY_MAX - current.familiarity) * FAMILIARITY_GAIN;
        current.familiarity = TeamTacticalFamiliarity::hold(grown);
        current.matches_together = current.matches_together.saturating_add(1);
    } else {
        let kept = current.familiarity * FAMILIARITY_KEPT_ON_CHANGE;
        *current = TeamTacticalFamiliarity::new(team.formation.clone(), kept, 1);
    }
}

/// What the assistant can say about a side's understanding, by kind; the screen words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdviceKind {
    /// Two starters who understand each other well: keep them together.
    StrongPartnership,
    /// Two starters who have not clicked on the pitch.
    WeakPartnership,
    /// The side does not yet know the shape it plays.
    UnfamiliarShape,
}

/// One piece of advice, by ids and numbers only: the players it is about (none for the shape)
/// and the bond or familiarity behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct AssistantAdvice {
    pub kind: AdviceKind,
    pub player_ids: Vec<String>,
    pub value: i32,
}

/// A pair bond at or above this is a partnership worth keeping.
const STRONG_BOND: i8 = 40;
/// A pair bond at or below this is one that has not clicked.
const WEAK_BOND: i8 = -20;
/// A familiarity below this is a shape the side does not yet know.
const UNFAMILIAR_BELOW: f32 = 40.0;
/// The most partnerships of each kind the assistant names.
const PAIRS_NAMED: usize = 3;

/// The assistant's advice on the eleven `team` would field: its strongest and weakest
/// partnerships among `xi_players`, and the shape when the side does not know it.
pub fn assistant_chemistry_advice(
    pairs: &[PairChemistry],
    team: &Team,
    xi_players: &[&Player],
) -> Vec<AssistantAdvice> {
    let xi: HashSet<&str> = xi_players.iter().map(|p| p.id.as_str()).collect();
    let mut within: Vec<&PairChemistry> = pairs
        .iter()
        .filter(|pair| xi.contains(pair.player_a.as_str()) && xi.contains(pair.player_b.as_str()))
        .collect();
    within.sort_by(|a, b| b.bond.cmp(&a.bond).then(a.player_a.cmp(&b.player_a)));
    let advice_for = |kind, pair: &PairChemistry| AssistantAdvice {
        kind,
        player_ids: vec![pair.player_a.clone(), pair.player_b.clone()],
        value: i32::from(pair.bond),
    };
    let mut advice: Vec<_> = within
        .iter()
        .filter(|pair| pair.bond >= STRONG_BOND)
        .take(PAIRS_NAMED)
        .map(|pair| advice_for(AdviceKind::StrongPartnership, pair))
        .collect();
    advice.extend(
        within
            .iter()
            .rev()
            .filter(|pair| pair.bond <= WEAK_BOND)
            .take(PAIRS_NAMED)
            .map(|pair| advice_for(AdviceKind::WeakPartnership, pair)),
    );
    let familiarity = &team.tactical_familiarity;
    let known = familiarity.formation == team.formation;
    if !familiarity.formation.is_empty() && (!known || familiarity.familiarity < UNFAMILIAR_BELOW) {
        let value = if known { familiarity.familiarity } else { 0.0 };
        advice.push(AssistantAdvice {
            kind: AdviceKind::UnfamiliarShape,
            player_ids: Vec::new(),
            value: value.round() as i32,
        });
    }
    advice
}

fn canonical_key(first: &str, second: &str) -> (String, String) {
    let (a, b) = PairChemistry::canonical(first, second);
    (a.to_string(), b.to_string())
}

fn apply_changes(pairs: &mut Vec<PairChemistry>, changes: HashMap<(String, String), (u32, i16)>) {
    let mut fresh = Vec::new();
    for ((a, b), (minutes, bond)) in changes {
        let found = pairs.binary_search_by(|pair| {
            (pair.player_a.as_str(), pair.player_b.as_str()).cmp(&(a.as_str(), b.as_str()))
        });
        match found {
            Ok(index) => {
                let pair = &mut pairs[index];
                pair.shared_minutes = pair.shared_minutes.saturating_add(minutes);
                pair.adjust_bond(bond);
            }
            Err(_) => {
                let mut pair = PairChemistry::new(&a, &b, minutes, 0);
                pair.adjust_bond(bond);
                fresh.push(pair);
            }
        }
    }
    if !fresh.is_empty() {
        pairs.extend(fresh);
        // A stable sort finds the two sorted runs and merges them, so this is linear.
        pairs.sort_by(|x, y| (&x.player_a, &x.player_b).cmp(&(&y.player_a, &y.player_b)));
    }
}

fn pair_between<'p>(
    pairs: &'p [PairChemistry],
    first: &str,
    second: &str,
) -> Option<&'p PairChemistry> {
    let (a, b) = PairChemistry::canonical(first, second);
    pairs
        .binary_search_by(|pair| (pair.player_a.as_str(), pair.player_b.as_str()).cmp(&(a, b)))
        .ok()
        .map(|index| &pairs[index])
}

fn birth_year(player: &Player) -> Option<i32> {
    player.date_of_birth.get(..4)?.parse().ok()
}

fn of_an_age(first: &Player, second: &Player) -> bool {
    match (birth_year(first), birth_year(second)) {
        (Some(a), Some(b)) => (a - b).abs() <= AGE_GROUP_YEARS,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::uniform_attributes;
    use domain::player::Position;
    use std::collections::HashMap;

    fn player(id: &str, nation: &str, born: &str) -> Player {
        let mut p = Player::new(
            id.into(),
            id.into(),
            id.into(),
            born.into(),
            nation.into(),
            Position::CentralMidfielder,
            uniform_attributes(60),
        );
        p.football_nation = nation.into();
        p
    }

    fn report(minutes: &[(&str, u8)], goals: &[(&str, Option<&str>)]) -> engine::MatchReport {
        let player_stats: HashMap<_, _> = minutes
            .iter()
            .map(|(id, played)| {
                (
                    id.to_string(),
                    engine::PlayerMatchStats {
                        minutes_played: *played,
                        ..Default::default()
                    },
                )
            })
            .collect();
        engine::MatchReport {
            goals: goals
                .iter()
                .map(|(scorer, assister)| engine::GoalDetail {
                    minute: 10,
                    scorer_id: scorer.to_string(),
                    assist_id: assister.map(str::to_string),
                    goal_source: engine::GoalSource::OpenPlay,
                    side: engine::Side::Home,
                })
                .collect(),
            player_stats,
            home_goals: 0,
            away_goals: 0,
            home_stats: engine::TeamStats::default(),
            away_stats: engine::TeamStats::default(),
            events: Vec::new(),
            home_possession: 50.0,
            total_minutes: 90,
            home_penalties: None,
            away_penalties: None,
        }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| id.to_string()).collect()
    }

    /// Given three team-mates, one subbed on for the last 30 minutes, and a goal one made for
    /// another
    /// When the match is recorded
    /// Then each pair shares the shorter of their minutes, builds bond by it, and the goal pair
    ///      builds more; pairs are stored once, in canonical order
    #[test]
    fn a_match_builds_shared_minutes_and_bond_between_team_mates() {
        let match_report = report(&[("b", 90), ("a", 90), ("c", 30)], &[("b", Some("a"))]);
        let mut pairs = Vec::new();

        record_match_chemistry(
            &mut pairs,
            &match_report,
            &[ids(&["b", "a", "c"])],
            &goal_assist_pairs(&match_report),
        );

        assert_eq!(
            pairs,
            vec![
                PairChemistry::new("a", "b", 90, 2 + 3),
                PairChemistry::new("a", "c", 30, 0),
                PairChemistry::new("b", "c", 30, 0),
            ]
        );
    }

    /// Given a pair already on record and two players of the other side
    /// When a second match is recorded
    /// Then the pair adds to what it had, and the two sides never pair across
    #[test]
    fn a_second_match_adds_to_a_pair_and_sides_never_pair_across() {
        let match_report = report(&[("a", 90), ("b", 90), ("x", 90), ("y", 90)], &[]);
        let mut pairs = vec![PairChemistry::new("a", "b", 180, 4)];

        record_match_chemistry(
            &mut pairs,
            &match_report,
            &[ids(&["a", "b"]), ids(&["x", "y"])],
            &[],
        );

        assert_eq!(
            pairs,
            vec![
                PairChemistry::new("a", "b", 270, 6),
                PairChemistry::new("x", "y", 90, 2)
            ]
        );
    }

    /// Given a side that won, one beaten, and one beaten heavily, each of two who played 90 minutes
    /// When the match is recorded for each
    /// Then a full match builds 2, plus 1 for the win, less 2 for the defeat, less 3 for the rout
    #[test]
    fn the_result_moves_the_bond_of_every_pair_of_a_side() {
        let mut won = report(&[("a", 90), ("b", 90), ("x", 90), ("y", 90)], &[]);
        won.home_goals = 2;
        won.away_goals = 1;
        let mut pairs = Vec::new();
        record_match_chemistry(&mut pairs, &won, &[ids(&["a", "b"]), ids(&["x", "y"])], &[]);
        assert_eq!(pairs[0], PairChemistry::new("a", "b", 90, 3));
        assert_eq!(pairs[1], PairChemistry::new("x", "y", 90, 0));

        let mut routed = won.clone();
        routed.home_goals = 4;
        routed.away_goals = 0;
        let mut pairs = Vec::new();
        record_match_chemistry(
            &mut pairs,
            &routed,
            &[ids(&["a", "b"]), ids(&["x", "y"])],
            &[],
        );
        assert_eq!(pairs[1], PairChemistry::new("x", "y", 90, -1));
    }

    /// Given pairs of team-mates, and one whose player has left for another club
    /// When the pairs are kept to team-mates
    /// Then only the pair still at one club remains
    #[test]
    fn a_pair_goes_when_one_of_them_leaves_the_club() {
        let mut a = player("a", "ENG", "1995-01-01");
        let mut b = player("b", "ENG", "1995-01-01");
        let mut c = player("c", "ENG", "1995-01-01");
        a.team_id = Some("t1".into());
        b.team_id = Some("t1".into());
        c.team_id = Some("t2".into());
        let mut pairs = vec![
            PairChemistry::new("a", "b", 90, 2),
            PairChemistry::new("a", "c", 90, 2),
        ];

        retain_team_mates(&mut pairs, &[a, b, c]);

        assert_eq!(pairs, vec![PairChemistry::new("a", "b", 90, 2)]);
    }

    /// Given an eleven that has not played together, one bonded to the limit and of one nation
    /// and age, and one bonded at the floor and of nothing in common
    /// When their cohesion is read
    /// Then the first is neutral, and the others sit at the two ends of 0.96..=1.04
    #[test]
    fn cohesion_is_neutral_for_strangers_and_bounded_at_both_ends() {
        let squad: Vec<Player> = (0..4)
            .map(|i| {
                let (nation, born) = if i < 2 {
                    ("ENG", "1995-01-01")
                } else {
                    ("FRA", "2005-01-01")
                };
                player(&format!("p{i}"), nation, born)
            })
            .collect();
        let xi: Vec<&Player> = squad.iter().collect();
        // A side that has not played together reads neutral whatever its nations and ages.
        let neutral_ish = team_cohesion_factor(&[], &xi);
        assert_eq!(neutral_ish, 1.0);

        let one_nation: Vec<Player> = (0..4)
            .map(|i| player(&format!("q{i}"), "ENG", "1995-01-01"))
            .collect();
        let one: Vec<&Player> = one_nation.iter().collect();
        let mut bonded = Vec::new();
        let mut apart = Vec::new();
        for (i, first) in one.iter().enumerate() {
            for second in &one[i + 1..] {
                bonded.push(PairChemistry::new(&first.id, &second.id, 900, 100));
                apart.push(PairChemistry::new(&first.id, &second.id, 900, -100));
            }
        }
        bonded.sort_by(|x, y| (&x.player_a, &x.player_b).cmp(&(&y.player_a, &y.player_b)));
        apart.sort_by(|x, y| (&x.player_a, &x.player_b).cmp(&(&y.player_a, &y.player_b)));
        let high = team_cohesion_factor(&bonded, &one);
        assert!((high - 1.035).abs() < 1e-9, "{high}");
        let mixed: Vec<Player> = (0..4)
            .map(|i| {
                player(
                    &format!("q{i}"),
                    &format!("N{i}"),
                    &format!("{}-01-01", 1980 + 10 * i),
                )
            })
            .collect();
        let mixed_xi: Vec<&Player> = mixed.iter().collect();
        let low = team_cohesion_factor(&apart, &mixed_xi);
        assert!((low - 0.965).abs() < 1e-9, "{low}");
        for factor in [neutral_ish, high, low] {
            assert!((0.96..=1.04).contains(&factor));
        }
        assert_eq!(team_cohesion_factor(&bonded, &[]), 1.0);
    }

    fn team(formation: &str) -> Team {
        let mut team = Team::new(
            "t".into(),
            "T".into(),
            "T".into(),
            "ENG".into(),
            "City".into(),
            "Ground".into(),
            10_000,
        );
        team.formation = formation.into();
        team
    }

    /// Given a side first seen, then playing the same shape, then changing shape
    /// When familiarity is updated after each match
    /// Then it starts at the middle, grows toward full, and keeps part of itself on a change
    #[test]
    fn familiarity_starts_at_the_middle_grows_and_falls_on_a_change_of_shape() {
        let mut side = team("4-4-2");
        update_tactical_familiarity(&mut side);
        assert_eq!(
            side.tactical_familiarity,
            TeamTacticalFamiliarity::new("4-4-2", 50.0, 1)
        );
        update_tactical_familiarity(&mut side);
        assert!((side.tactical_familiarity.familiarity - 54.0).abs() < 1e-4);
        assert_eq!(side.tactical_familiarity.matches_together, 2);

        side.formation = "4-3-3".into();
        update_tactical_familiarity(&mut side);
        assert_eq!(side.tactical_familiarity.formation, "4-3-3");
        assert!((side.tactical_familiarity.familiarity - 21.6).abs() < 1e-4);
        assert_eq!(side.tactical_familiarity.matches_together, 1);
    }

    /// Given a side not yet seen, one at the middle, one fully familiar, and one in a new shape
    /// When the match factor of its familiarity is read
    /// Then it is 1.0, 1.0, 1.02, and below 1.0
    #[test]
    fn familiarity_moves_play_by_at_most_two_percent() {
        let unseen = TeamTacticalFamiliarity::default();
        assert_eq!(familiarity_match_factor(&unseen, "4-4-2"), 1.0);
        let middle = TeamTacticalFamiliarity::new("4-4-2", 50.0, 3);
        assert!((familiarity_match_factor(&middle, "4-4-2") - 1.0).abs() < 1e-12);
        let full = TeamTacticalFamiliarity::new("4-4-2", 100.0, 30);
        assert!((familiarity_match_factor(&full, "4-4-2") - 1.02).abs() < 1e-12);
        assert!(familiarity_match_factor(&middle, "3-5-2") < 1.0);
        let none = TeamTacticalFamiliarity::new("4-4-2", 0.0, 1);
        assert!((familiarity_match_factor(&none, "4-4-2") - 0.98).abs() < 1e-12);
    }

    /// Given an eleven with one strong partnership, one that has not clicked, and a side that
    /// does not know its shape
    /// When the assistant advises
    /// Then it names the strong pair, the weak pair and the shape, by ids and numbers alone
    #[test]
    fn the_assistant_names_partnerships_and_an_unfamiliar_shape() {
        let squad: Vec<Player> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| player(id, "ENG", "1995-01-01"))
            .collect();
        let xi: Vec<&Player> = squad.iter().collect();
        let pairs = vec![
            PairChemistry::new("a", "b", 900, 60),
            PairChemistry::new("a", "c", 90, 5),
            PairChemistry::new("c", "d", 90, -30),
            PairChemistry::new("d", "z", 900, 90),
        ];
        let mut side = team("4-4-2");
        side.tactical_familiarity = TeamTacticalFamiliarity::new("4-4-2", 20.0, 2);

        let advice = assistant_chemistry_advice(&pairs, &side, &xi);

        assert_eq!(
            advice,
            vec![
                AssistantAdvice {
                    kind: AdviceKind::StrongPartnership,
                    player_ids: ids(&["a", "b"]),
                    value: 60
                },
                AssistantAdvice {
                    kind: AdviceKind::WeakPartnership,
                    player_ids: ids(&["c", "d"]),
                    value: -30
                },
                AssistantAdvice {
                    kind: AdviceKind::UnfamiliarShape,
                    player_ids: Vec::new(),
                    value: 20
                },
            ]
        );
    }
}
