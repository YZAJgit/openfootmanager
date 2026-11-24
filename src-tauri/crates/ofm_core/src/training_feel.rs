//! M6 training feel: individual development plans, mentor character transfer and the multi-week
//! causal development digest.
//!
//! Three rules live here, all of them slow and few-clicks:
//!
//! 1. A club may run at most [`MAX_INDIVIDUAL_PLANS`] individual development plans at once. A plan
//!    is one player's focus, intensity and optional mentor, stored as a one-player entry in the
//!    club's existing `training_groups` list ([`plan_individual_development`]).
//! 2. A strong senior character paired with a young player moves the young player's character
//!    toward his own, one bounded step a week ([`mentor_week`], [`process_weekly_mentoring`]).
//! 3. The last N weeks of training, plans, mentoring and playing time become cause-and-effect
//!    lines a player reads in the inbox: translation keys and parameters, never prose
//!    ([`development_digest`]).
//!
//! Persistence rides the existing `teams.training_groups` JSON blob: the plan's `intensity` and
//! `mentor_id` are `serde(default)` fields on `TrainingGroup`, so a save written before them
//! still loads and no SQL migration is involved (v070 belongs to the youth-academy slice).

use std::collections::{BTreeMap, HashMap, HashSet};

use domain::character::CharacterProfile;
use domain::player::Player;
use domain::team::{Team, TrainingFocus, TrainingGroup, TrainingIntensity};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::character::assessed_character;
use crate::game::Game;
use crate::stable_hash::stable_hash;

/// The most individual development plans one club may run at once. A fourth is rejected with
/// [`INDIVIDUAL_PLAN_LIMIT_ERROR`]: three is as much bespoke attention as a coaching staff gives.
pub const MAX_INDIVIDUAL_PLANS: usize = 3;

/// The translation key a club sees when it asks for a fourth plan.
pub const INDIVIDUAL_PLAN_LIMIT_ERROR: &str = "be.error.training.individual_plan_limit";
/// Translation key when a training focus string does not match a known [`TrainingFocus`].
pub const INVALID_FOCUS_ERROR: &str = "be.error.training.invalid_focus";
/// Translation key when a training intensity string does not match a known [`TrainingIntensity`].
pub const INVALID_INTENSITY_ERROR: &str = "be.error.training.invalid_intensity";
/// Translation key when a plan names the player himself as his own mentor.
pub const MENTOR_SAME_PLAYER_ERROR: &str = "be.error.training.mentor_same_player";
/// Translation key when the chosen mentor does not belong to the user's club.
pub const MENTOR_NOT_IN_CLUB_ERROR: &str = "be.error.training.mentor_not_in_club";
/// Translation key when the chosen mentor does not meet the age or character bar.
pub const MENTOR_INELIGIBLE_ERROR: &str = "be.error.training.mentor_ineligible";
/// Translation key when the target player is past the mentoring age ceiling.
pub const MENTEE_INELIGIBLE_ERROR: &str = "be.error.training.mentee_ineligible";

/// The id prefix that fans a plan's group out from the squad groups it shares a list with.
const INDIVIDUAL_PLAN_ID_PREFIX: &str = "individual:";

/// A mentor's example is a senior player's, not a coaching session.
pub const MENTOR_MIN_AGE: u32 = 30;
/// A mentee is a player still shaping his habits.
pub const MENTEE_MAX_AGE: u32 = 21;
/// Professionalism is the habit a mentor's example transfers best, so a mentor below this is not
/// a mentor whatever his age.
pub const MENTOR_MIN_PROFESSIONALISM: u8 = 70;
/// Ambition backs it up: a professional with no drive teaches a young player to be comfortable.
pub const MENTOR_MIN_AMBITION: u8 = 60;

/// A trait closer than this to the mentor's is not worth a step: two points apart is already the
/// same habit.
pub const MENTOR_MIN_GAP: u8 = 2;

/// The chance, per trait per week, that a trait moves one point. At 60 a six-point gap closes in
/// about ten weeks, which is why the digest reads a multi-week window rather than one session.
const MENTOR_WEEKLY_PERCENT: u64 = 60;

/// A full competitive match: this many minutes is playing time the digest reports as its own
/// cause rather than as part of a training week.
pub const MIN_DIGEST_MINUTES: u16 = 90;

/// One individual development plan, read from the club's group list. Borrowed so a caller can
/// look without copying the club.
#[derive(Debug, Clone, PartialEq)]
pub struct IndividualPlan<'a> {
    pub player_id: &'a str,
    pub focus: &'a TrainingFocus,
    pub intensity: &'a TrainingIntensity,
    pub mentor_id: Option<&'a str>,
}

/// Whether `group` is an individual development plan: one player and an intensity of its own.
///
/// A one-player squad group without an intensity is an ordinary group, not a plan, so the group
/// editor's own one-player groups do not count against the club's three.
pub fn is_individual_plan(group: &TrainingGroup) -> bool {
    group.player_ids.len() == 1 && group.intensity.is_some()
}

/// Every individual plan the club is running, in the order they are stored.
pub fn individual_plans(team: &Team) -> Vec<IndividualPlan<'_>> {
    team.training_groups
        .iter()
        .filter(|group| is_individual_plan(group))
        .filter_map(|group| {
            let player_id = group.player_ids.first()?.as_str();
            let intensity = group.intensity.as_ref()?;
            Some(IndividualPlan {
                player_id,
                focus: &group.focus,
                intensity,
                mentor_id: group.mentor_id.as_deref(),
            })
        })
        .collect()
}

/// The plan the club runs for `player_id`, if any.
pub fn individual_plan_for<'a>(team: &'a Team, player_id: &str) -> Option<IndividualPlan<'a>> {
    individual_plans(team)
        .into_iter()
        .find(|plan| plan.player_id == player_id)
}

/// Give one player his own development plan, or retune the one he already has.
///
/// A plan already running for the same player is updated in place and does not count against the
/// limit again. A fourth plan is rejected with [`INDIVIDUAL_PLAN_LIMIT_ERROR`], and the club is
/// left exactly as it was.
pub fn plan_individual_development(
    team: &mut Team,
    player_id: &str,
    player_name: &str,
    focus: TrainingFocus,
    intensity: TrainingIntensity,
    mentor_id: Option<String>,
) -> Result<(), String> {
    let mentor = mentor_id.filter(|mentor| mentor != player_id);
    if let Some(existing) = team
        .training_groups
        .iter_mut()
        .find(|group| is_individual_plan(group) && names_player(group, player_id))
    {
        existing.focus = focus;
        existing.intensity = Some(intensity);
        existing.mentor_id = mentor;
        return Ok(());
    }
    if individual_plans(team).len() >= MAX_INDIVIDUAL_PLANS {
        return Err(INDIVIDUAL_PLAN_LIMIT_ERROR.to_string());
    }
    team.training_groups.push(TrainingGroup {
        id: format!("{INDIVIDUAL_PLAN_ID_PREFIX}{player_id}"),
        name: player_name.to_string(),
        focus,
        player_ids: vec![player_id.to_string()],
        intensity: Some(intensity),
        mentor_id: mentor,
    });
    Ok(())
}

/// End the plan the club runs for `player_id`. Returns whether there was one.
pub fn clear_individual_plan(team: &mut Team, player_id: &str) -> bool {
    let before = team.training_groups.len();
    team.training_groups
        .retain(|group| !(is_individual_plan(group) && names_player(group, player_id)));
    team.training_groups.len() != before
}

/// Whether an individual plan's one player is `player_id`.
fn names_player(group: &TrainingGroup, player_id: &str) -> bool {
    group.player_ids.first().map(String::as_str) == Some(player_id)
}

/// The group list after an edit in the squad-group editor.
///
/// The editor sends the whole list back, and it does not know about individual plans (no screen
/// shows them yet). Keeping the plans the incoming list does not name is what stops a manager
/// editing a squad group from silently deleting three players' plans.
pub fn merge_training_groups(
    existing: &[TrainingGroup],
    mut incoming: Vec<TrainingGroup>,
) -> Vec<TrainingGroup> {
    let plan_ids: HashSet<&str> = existing
        .iter()
        .filter(|group| is_individual_plan(group))
        .map(|group| group.id.as_str())
        .collect();
    // The editor cannot make or edit a plan: a group it sends under a plan's id, or one named like a
    // plan, is not its to write.
    incoming.retain(|group| {
        !plan_ids.contains(group.id.as_str()) && !group.id.starts_with(INDIVIDUAL_PLAN_ID_PREFIX)
    });
    incoming.extend(
        existing
            .iter()
            .filter(|group| is_individual_plan(group))
            .cloned(),
    );
    incoming
}

/// Whether a profile is one a young player should be picking habits up from.
pub fn mentor_is_strong(profile: &CharacterProfile) -> bool {
    profile.professionalism >= MENTOR_MIN_PROFESSIONALISM && profile.ambition >= MENTOR_MIN_AMBITION
}

/// Whether the two ages and the mentor's character make a mentoring pair.
pub fn can_mentor(mentor_age: u32, mentee_age: u32, mentor: &CharacterProfile) -> bool {
    mentor_age >= MENTOR_MIN_AGE && mentee_age <= MENTEE_MAX_AGE && mentor_is_strong(mentor)
}

/// One club player who qualifies as a mentor or mentee under the club's mentoring rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct IndividualPlanCandidate {
    pub id: String,
    pub match_name: String,
    pub age: u32,
}

/// Backend policy and eligible mentoring candidates for a club's individual development plans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct IndividualPlanOptions {
    pub max_plans: usize,
    pub eligible_mentors: Vec<IndividualPlanCandidate>,
    pub eligible_mentees: Vec<IndividualPlanCandidate>,
}

/// Eligible mentors, eligible mentees and plan cap for `team_id` on the game's current date.
pub fn individual_plan_options(game: &Game, team_id: &str) -> IndividualPlanOptions {
    let date = game.clock.current_date.date_naive();
    let mut eligible_mentors = Vec::new();
    let mut eligible_mentees = Vec::new();

    for player in &game.players {
        if player.retired || player.team_id.as_deref() != Some(team_id) {
            continue;
        }
        let age = domain::age::playing_age(date, &player.date_of_birth);
        let candidate = || IndividualPlanCandidate {
            id: player.id.clone(),
            match_name: player.match_name.clone(),
            age,
        };
        if age >= MENTOR_MIN_AGE && mentor_is_strong(&assessed_character(player, game.seed)) {
            eligible_mentors.push(candidate());
        }
        if age <= MENTEE_MAX_AGE {
            eligible_mentees.push(candidate());
        }
    }

    let sort_candidates = |list: &mut Vec<IndividualPlanCandidate>| {
        list.sort_by(|left, right| {
            left.match_name
                .cmp(&right.match_name)
                .then_with(|| left.id.cmp(&right.id))
        });
    };
    sort_candidates(&mut eligible_mentors);
    sort_candidates(&mut eligible_mentees);

    IndividualPlanOptions {
        max_plans: MAX_INDIVIDUAL_PLANS,
        eligible_mentors,
        eligible_mentees,
    }
}

/// One week of mentoring: each transferable trait of `mentee` moves at most one point toward
/// `mentor`'s, and never past it.
///
/// `pair_key` names the pair (a plan's mentee and mentor) and `week` numbers the week, so the
/// same pair in the same week of the same world always moves the same way, and two pairs do not
/// share a draw. Injury proneness is not transferable: a young player does not catch a veteran's
/// fragility.
pub fn mentor_week(
    mentor: &CharacterProfile,
    mentee: &CharacterProfile,
    pair_key: &str,
    world_seed: u64,
    week: u32,
) -> CharacterProfile {
    let step =
        |name: &str, from: u8, to: u8| shift_toward(name, from, to, pair_key, world_seed, week);
    CharacterProfile {
        professionalism: step(
            "professionalism",
            mentor.professionalism,
            mentee.professionalism,
        ),
        ambition: step("ambition", mentor.ambition, mentee.ambition),
        loyalty: step("loyalty", mentor.loyalty, mentee.loyalty),
        consistency: step("consistency", mentor.consistency, mentee.consistency),
        big_matches: step("big_matches", mentor.big_matches, mentee.big_matches),
        pressure_handling: step(
            "pressure_handling",
            mentor.pressure_handling,
            mentee.pressure_handling,
        ),
        adaptability: step("adaptability", mentor.adaptability, mentee.adaptability),
        injury_proneness: mentee.injury_proneness,
    }
    .clamped()
}

/// One trait of the mentee after a week: at most a point toward the mentor, never past him.
fn shift_toward(
    trait_name: &str,
    mentor: u8,
    mentee: u8,
    pair_key: &str,
    world_seed: u64,
    week: u32,
) -> u8 {
    if mentor.abs_diff(mentee) < MENTOR_MIN_GAP {
        return mentee;
    }
    let key = format!("{pair_key}/{trait_name}/{week}");
    if stable_hash(key.as_bytes(), world_seed) % 100 >= MENTOR_WEEKLY_PERCENT {
        return mentee;
    }
    if mentor > mentee {
        mentee.saturating_add(1).min(mentor)
    } else {
        mentee.saturating_sub(1).max(mentor)
    }
}

/// A mentoring pair found on one club's plan list.
struct MentorPair {
    mentor: usize,
    mentee: usize,
    pair_key: String,
}

/// Advance every club's mentoring by one week. Returns how many mentees moved.
///
/// Called once a week from the training ground (`turn::run_training_ground`), never daily: a
/// character is a habit, and habits are not rescheduled on a Wednesday.
pub fn process_weekly_mentoring(game: &mut Game) -> usize {
    let date = game.clock.current_date.date_naive();
    let week = week_of(date);
    let pairs = mentor_pairs(game, date);
    // Persist the assessment first: moving a profile that is only derived on the fly would write
    // the whole derived character plus a step, so every trait would change in one week.
    for pair in &pairs {
        for index in [pair.mentor, pair.mentee] {
            crate::character::assess_unassessed_characters(
                std::slice::from_mut(&mut game.players[index]),
                game.seed,
            );
        }
    }

    let mut updates: Vec<(usize, CharacterProfile)> = Vec::new();
    for pair in pairs {
        let mentor = assessed_character(&game.players[pair.mentor], game.seed);
        let mentee = assessed_character(&game.players[pair.mentee], game.seed);
        let next = mentor_week(&mentor, &mentee, &pair.pair_key, game.seed, week);
        if next != mentee {
            updates.push((pair.mentee, next));
        }
    }
    let moved = updates.len();
    for (index, profile) in updates {
        game.players[index].character = profile;
    }
    moved
}

/// Every mentoring pair in the game on `date`: a plan whose mentor is an eligible senior at the
/// same club as the young player the plan names.
fn mentor_pairs(game: &Game, date: chrono::NaiveDate) -> Vec<MentorPair> {
    let index: HashMap<&str, usize> = game
        .players
        .iter()
        .enumerate()
        .map(|(at, player)| (player.id.as_str(), at))
        .collect();
    let mut pairs = Vec::new();
    for team in &game.teams {
        for plan in individual_plans(team) {
            let Some(mentor_id) = plan.mentor_id else {
                continue;
            };
            let (Some(&mentor), Some(&mentee)) = (index.get(mentor_id), index.get(plan.player_id))
            else {
                continue;
            };
            if mentor == mentee {
                continue;
            }
            let mentor_player = &game.players[mentor];
            let mentee_player = &game.players[mentee];
            let at_this_club =
                |player: &Player| player.team_id.as_deref() == Some(team.id.as_str());
            if !at_this_club(mentor_player) || !at_this_club(mentee_player) {
                continue;
            }
            let mentor_age = domain::age::playing_age(date, &mentor_player.date_of_birth);
            let mentee_age = domain::age::playing_age(date, &mentee_player.date_of_birth);
            let character = assessed_character(mentor_player, game.seed);
            if !can_mentor(mentor_age, mentee_age, &character) {
                continue;
            }
            pairs.push(MentorPair {
                mentor,
                mentee,
                pair_key: format!("{}/{}", plan.player_id, mentor_id),
            });
        }
    }
    pairs
}

/// The week a date falls in, counted from the epoch: a number a replay of the save repeats.
fn week_of(date: chrono::NaiveDate) -> u32 {
    use chrono::Datelike;
    u32::try_from(date.num_days_from_ce().max(0) / 7).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Development digest
// ---------------------------------------------------------------------------

/// One week of a player's training, plan, mentoring and playing time, as the digest reads it.
///
/// A caller hands over the weeks it has (the inbox digest keeps six to eight) and the function
/// reads the last ones. Every field is `serde(default)` so a record written before a field
/// existed still reads, and a missing week reads as a week with nothing in it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevelopmentWeek {
    /// The club's standing focus that week.
    pub focus: TrainingFocus,
    /// Sessions he actually trained in that week.
    pub sessions: u8,
    /// The individual plan's focus, when he was on a plan.
    pub plan_focus: Option<TrainingFocus>,
    /// Consecutive weeks with a mentor, as the caller counts them.
    pub mentor_weeks: u16,
    /// Minutes of competitive football he played.
    pub minutes_played: u16,
    /// Rating points he moved that week, up or down.
    pub rating_change: i8,
}

/// One cause-and-effect line: what happened, what it did, and the numbers behind it.
///
/// `cause_key` and `effect_key` are translation keys (`be.digest.*`); a screen renders them and
/// fills `params`. No English prose ever leaves this module.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevelopmentDigestLine {
    pub cause_key: String,
    pub effect_key: String,
    pub params: BTreeMap<String, String>,
}

/// One player's digest: the lines in a fixed order, and the net rating move they explain.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerDevelopmentDigest {
    pub player_id: String,
    /// How many weeks were actually read (the window, capped at what was handed over).
    pub weeks: usize,
    pub net_rating_change: i16,
    pub lines: Vec<DevelopmentDigestLine>,
}

/// A cause line: the individual plan carried the week.
pub const CAUSE_INDIVIDUAL_PLAN: &str = "be.digest.cause.individual_plan";
/// A cause line: a mentor was at work.
pub const CAUSE_MENTOR: &str = "be.digest.cause.mentor";
/// A cause line: match minutes, more than a week's training.
pub const CAUSE_MATCH_MINUTES: &str = "be.digest.cause.match_minutes";
/// A cause line: ordinary squad training.
pub const CAUSE_TRAINING_LOAD: &str = "be.digest.cause.training_load";
/// A cause line: he neither trained nor played.
pub const CAUSE_IDLE: &str = "be.digest.cause.idle";
/// An effect line: his rating rose.
pub const EFFECT_RATING_GAIN: &str = "be.digest.effect.rating_gain";
/// An effect line: his rating fell.
pub const EFFECT_RATING_LOSS: &str = "be.digest.effect.rating_loss";
/// An effect line: his rating held.
pub const EFFECT_RATING_HELD: &str = "be.digest.effect.rating_held";
/// An effect line: his character moved toward his mentor's.
pub const EFFECT_CHARACTER_SHIFT: &str = "be.digest.effect.character_shift";
/// An effect line: nothing moved.
pub const EFFECT_STALLED: &str = "be.digest.effect.stalled";

/// The last `window` weeks as cause-and-effect lines for one player.
///
/// Deterministic: the same weeks in the same order give the same lines, because nothing is read
/// from a map without an order and the causes are emitted in a fixed sequence. A window of zero
/// or no weeks at all gives an empty digest, not a line about nothing.
pub fn development_digest(
    player_id: &str,
    weeks: &[DevelopmentWeek],
    window: usize,
) -> PlayerDevelopmentDigest {
    let start = weeks.len().saturating_sub(window);
    let read = if window == 0 {
        &[][..]
    } else {
        &weeks[start..]
    };
    let net: i16 = read.iter().map(|week| i16::from(week.rating_change)).sum();
    let sessions: u32 = read.iter().map(|week| u32::from(week.sessions)).sum();
    let minutes: u32 = read.iter().map(|week| u32::from(week.minutes_played)).sum();
    let plan_weeks = read.iter().filter(|week| week.plan_focus.is_some()).count();
    let mentor_weeks = read.iter().filter(|week| week.mentor_weeks > 0).count();

    let mut lines = Vec::new();
    if let Some(focus) = dominant_plan_focus(read) {
        lines.push(DevelopmentDigestLine {
            cause_key: CAUSE_INDIVIDUAL_PLAN.to_string(),
            effect_key: rating_effect(net).to_string(),
            params: params(&[
                ("focus", focus_name(&focus)),
                ("weeks", plan_weeks.to_string()),
                ("sessions", sessions.to_string()),
                ("change", net.to_string()),
            ]),
        });
    } else if minutes >= u32::from(MIN_DIGEST_MINUTES) {
        lines.push(DevelopmentDigestLine {
            cause_key: CAUSE_MATCH_MINUTES.to_string(),
            effect_key: rating_effect(net).to_string(),
            params: params(&[
                ("minutes", minutes.to_string()),
                ("change", net.to_string()),
            ]),
        });
    } else if sessions > 0 {
        lines.push(DevelopmentDigestLine {
            cause_key: CAUSE_TRAINING_LOAD.to_string(),
            effect_key: rating_effect(net).to_string(),
            params: params(&[
                ("sessions", sessions.to_string()),
                ("weeks", read.len().to_string()),
                ("change", net.to_string()),
            ]),
        });
    } else if !read.is_empty() {
        lines.push(DevelopmentDigestLine {
            cause_key: CAUSE_IDLE.to_string(),
            effect_key: EFFECT_STALLED.to_string(),
            params: params(&[("weeks", read.len().to_string())]),
        });
    }

    if mentor_weeks > 0 {
        lines.push(DevelopmentDigestLine {
            cause_key: CAUSE_MENTOR.to_string(),
            effect_key: EFFECT_CHARACTER_SHIFT.to_string(),
            params: params(&[("weeks", mentor_weeks.to_string())]),
        });
    }

    PlayerDevelopmentDigest {
        player_id: player_id.to_string(),
        weeks: read.len(),
        net_rating_change: net,
        lines,
    }
}

/// The focus the window spent most of its plan weeks on; the first seen wins a tie.
fn dominant_plan_focus(weeks: &[DevelopmentWeek]) -> Option<TrainingFocus> {
    let mut counts: Vec<(TrainingFocus, usize)> = Vec::new();
    for focus in weeks.iter().filter_map(|week| week.plan_focus.as_ref()) {
        match counts.iter_mut().find(|(seen, _)| seen == focus) {
            Some((_, count)) => *count += 1,
            None => counts.push((focus.clone(), 1)),
        }
    }
    counts
        .into_iter()
        .fold(
            None,
            |best: Option<(TrainingFocus, usize)>, entry| match best {
                Some((_, best_count)) if best_count >= entry.1 => best,
                _ => Some(entry),
            },
        )
        .map(|(focus, _)| focus)
}

/// The rating effect a net move stands for.
fn rating_effect(net: i16) -> &'static str {
    match net {
        change if change > 0 => EFFECT_RATING_GAIN,
        change if change < 0 => EFFECT_RATING_LOSS,
        _ => EFFECT_RATING_HELD,
    }
}

/// A focus as its stored variant name: the screen translates it, this module does not.
fn focus_name(focus: &TrainingFocus) -> String {
    format!("{focus:?}")
}

/// Params from a fixed list; a `BTreeMap` so the order a screen reads them in never depends on a
/// hash.
fn params(entries: &[(&str, String)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::GameClock;
    use crate::world::test_fixtures::{manager_for, nation_team, player_at};
    use chrono::{TimeZone, Utc};

    fn club(id: &str) -> Team {
        nation_team(id, "England", 500)
    }

    /// A player at `id` on `team_id`, born on `date_of_birth`, with habits worth changing.
    fn player(id: &str, team_id: &str, date_of_birth: &str) -> Player {
        let mut player = player_at(id, team_id, "2030-06-30");
        player.date_of_birth = date_of_birth.to_string();
        player.character = CharacterProfile {
            professionalism: 30,
            ambition: 30,
            loyalty: 40,
            consistency: 40,
            big_matches: 40,
            pressure_handling: 40,
            injury_proneness: 50,
            adaptability: 40,
        };
        player
    }

    fn strong_mentor(id: &str, team_id: &str, date_of_birth: &str) -> Player {
        let mut player = player(id, team_id, date_of_birth);
        player.character = CharacterProfile {
            professionalism: 90,
            ambition: 85,
            loyalty: 80,
            consistency: 80,
            big_matches: 75,
            pressure_handling: 80,
            injury_proneness: 30,
            adaptability: 80,
        };
        player
    }

    fn game_with(players: Vec<Player>) -> Game {
        let clock = GameClock::new(Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap());
        let mut game = Game::new(
            clock,
            manager_for("team1"),
            vec![club("team1")],
            players,
            vec![],
            vec![],
        );
        game.seed = 7;
        game
    }

    /// Given a player with no plan
    /// When the club gives him one with a focus, an intensity and a mentor
    /// Then the plan reads back with all three, and counts as an individual plan
    #[test]
    fn an_individual_plan_can_be_given_to_a_player() {
        let mut team = club("team1");
        plan_individual_development(
            &mut team,
            "p-1",
            "Young Player",
            TrainingFocus::Technical,
            TrainingIntensity::High,
            Some("p-2".to_string()),
        )
        .unwrap();

        let plan = individual_plan_for(&team, "p-1").unwrap();
        assert_eq!(plan.focus, &TrainingFocus::Technical);
        assert_eq!(plan.intensity, &TrainingIntensity::High);
        assert_eq!(plan.mentor_id, Some("p-2"));
        assert_eq!(team.training_groups.len(), 1);
        assert!(is_individual_plan(&team.training_groups[0]));
    }

    /// Given a club already running three individual plans
    /// When it asks for a fourth
    /// Then it is refused with the limit's translation key and the club keeps exactly three
    #[test]
    fn a_fourth_individual_plan_is_rejected_with_the_translation_key() {
        let mut team = club("team1");
        for at in 1..=MAX_INDIVIDUAL_PLANS {
            plan_individual_development(
                &mut team,
                &format!("p-{at}"),
                "Player",
                TrainingFocus::Physical,
                TrainingIntensity::Medium,
                None,
            )
            .unwrap();
        }

        let refused = plan_individual_development(
            &mut team,
            "p-4",
            "Fourth Player",
            TrainingFocus::Attacking,
            TrainingIntensity::High,
            None,
        );

        assert_eq!(refused, Err(INDIVIDUAL_PLAN_LIMIT_ERROR.to_string()));
        assert_eq!(individual_plans(&team).len(), MAX_INDIVIDUAL_PLANS);
        assert!(individual_plan_for(&team, "p-4").is_none());
        // Retuning a plan he already has is not a fourth plan.
        plan_individual_development(
            &mut team,
            "p-1",
            "Player",
            TrainingFocus::Defending,
            TrainingIntensity::Low,
            None,
        )
        .unwrap();
        assert_eq!(individual_plans(&team).len(), MAX_INDIVIDUAL_PLANS);
        assert_eq!(
            individual_plan_for(&team, "p-1").unwrap().focus,
            &TrainingFocus::Defending
        );
        // Ending one frees the slot for the player who was refused.
        assert!(clear_individual_plan(&mut team, "p-2"));
        plan_individual_development(
            &mut team,
            "p-4",
            "Fourth Player",
            TrainingFocus::Attacking,
            TrainingIntensity::High,
            None,
        )
        .unwrap();
        assert_eq!(individual_plans(&team).len(), MAX_INDIVIDUAL_PLANS);
    }

    /// Given a club whose squad groups are replaced by the group editor while a player is on a
    ///       plan
    /// When the merged list is written back
    /// Then the plan the editor never saw is still there
    #[test]
    fn the_group_editor_does_not_delete_an_individual_plan() {
        let mut team = club("team1");
        plan_individual_development(
            &mut team,
            "p-1",
            "Planned",
            TrainingFocus::Technical,
            TrainingIntensity::Medium,
            None,
        )
        .unwrap();
        let incoming = vec![TrainingGroup {
            id: "squad-group".to_string(),
            name: "Defenders".to_string(),
            focus: TrainingFocus::Defending,
            player_ids: vec!["p-2".to_string()],
            intensity: None,
            mentor_id: None,
        }];

        let merged = merge_training_groups(&team.training_groups, incoming);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "squad-group");
        assert_eq!(merged[1].player_ids, vec!["p-1".to_string()]);
        assert!(is_individual_plan(&merged[1]));
    }

    /// Given an editor round trip that omits private plan fields
    /// When it sends back a group with the plan's id
    /// Then the original plan and mentor remain intact.
    #[test]
    fn an_editor_round_trip_cannot_strip_a_plan() {
        let mut team = club("team1");
        plan_individual_development(
            &mut team,
            "p-1",
            "Planned",
            TrainingFocus::Technical,
            TrainingIntensity::High,
            Some("mentor".to_string()),
        )
        .unwrap();
        let mut incoming = team.training_groups.clone();
        incoming[0].intensity = None;
        incoming[0].mentor_id = None;
        let merged = merge_training_groups(&team.training_groups, incoming);
        assert_eq!(merged, team.training_groups);
    }

    /// Given a mentor's profile far from a young player's, and many weeks
    /// When the weeks run
    /// Then every trait moves toward the mentor's by at most one point a week, never past it,
    ///      and injury proneness does not transfer
    #[test]
    fn mentor_weeks_move_character_toward_the_mentor_within_the_weekly_bound() {
        let mentee = CharacterProfile {
            professionalism: 20,
            ambition: 30,
            loyalty: 90,
            consistency: 40,
            big_matches: 40,
            pressure_handling: 40,
            injury_proneness: 20,
            adaptability: 40,
        };
        let mentor = CharacterProfile {
            professionalism: 95,
            ambition: 90,
            loyalty: 40,
            consistency: 80,
            big_matches: 80,
            pressure_handling: 80,
            injury_proneness: 90,
            adaptability: 80,
        };

        let mut current = mentee;
        for week in 0..40 {
            let next = mentor_week(&mentor, &current, "mentee/mentor", 11, week);
            for (before, after, target) in [
                (
                    current.professionalism,
                    next.professionalism,
                    mentor.professionalism,
                ),
                (current.ambition, next.ambition, mentor.ambition),
                (current.loyalty, next.loyalty, mentor.loyalty),
                (current.consistency, next.consistency, mentor.consistency),
                (current.big_matches, next.big_matches, mentor.big_matches),
                (
                    current.pressure_handling,
                    next.pressure_handling,
                    mentor.pressure_handling,
                ),
                (current.adaptability, next.adaptability, mentor.adaptability),
            ] {
                assert!(before.abs_diff(after) <= 1, "week {week}");
                let between = before.min(target)..=before.max(target);
                assert!(
                    between.contains(&after),
                    "week {week}: {before} -> {after} past {target}"
                );
            }
            assert_eq!(next.injury_proneness, 20);
            current = next;
        }
        assert!(current.professionalism > mentee.professionalism);
        assert!(current.loyalty < mentee.loyalty);
        assert!(!mentor_is_strong(&CharacterProfile {
            professionalism: 50,
            ..mentor
        }));
    }

    /// Given a strong senior and a young player on an individual plan naming that mentor
    /// When the weeks pass
    /// Then the young player's professionalism climbs toward the mentor's, one point a week at
    ///      most, and a player with no mentor does not move
    #[test]
    fn a_strong_senior_mentor_moves_a_young_players_character() {
        let mentor = strong_mentor("mentor", "team1", "1990-01-01");
        let mentee = player("mentee", "team1", "2008-01-01");
        let unpaired = player("unpaired", "team1", "2008-01-01");
        let mentor_before = mentor.character;
        let mut game = game_with(vec![mentor, mentee, unpaired]);
        let mut team = game.teams.remove(0);
        plan_individual_development(
            &mut team,
            "mentee",
            "Mentee",
            TrainingFocus::Technical,
            TrainingIntensity::Medium,
            Some("mentor".to_string()),
        )
        .unwrap();
        game.teams.push(team);

        let mut previous = game.players[1].character.professionalism;
        for _ in 0..8 {
            process_weekly_mentoring(&mut game);
            let now = game.players[1].character.professionalism;
            assert!(now.abs_diff(previous) <= 1, "{previous} -> {now}");
            previous = now;
            game.clock.current_date += chrono::Duration::days(7);
        }

        assert!(game.players[1].character.professionalism > 30);
        assert!(game.players[1].character.professionalism <= 90);
        assert_eq!(game.players[0].character, mentor_before);
        assert_eq!(game.players[2].character.professionalism, 30);
    }

    fn club_with_plan(mentor: Option<&str>) -> Team {
        let mut team = nation_team("team1", "England", 500);
        plan_individual_development(
            &mut team,
            "mentee",
            "Mentee",
            TrainingFocus::Technical,
            TrainingIntensity::Medium,
            mentor.map(str::to_string),
        )
        .unwrap();
        team
    }

    /// Given a club with an individual plan
    /// When the planned player leaves, or only his mentor does
    /// Then the emptied plan is deleted and no longer counts, and a departed mentor is cleared.
    #[test]
    fn a_departing_player_or_mentor_leaves_no_stale_plan() {
        let mut team = club_with_plan(Some("mentor"));
        team.remove_player_references("mentor");
        assert_eq!(team.training_groups.len(), 1);
        assert_eq!(team.training_groups[0].mentor_id, None);

        team.remove_player_references("mentee");
        assert!(team.training_groups.is_empty());
    }

    /// Given a group editor that sends back a group under an individual plan's id
    /// When the list is merged
    /// Then the club's own plan stands and the editor's copy is dropped.
    #[test]
    fn the_group_editor_cannot_write_an_individual_plan() {
        let team = club_with_plan(None);
        let forged = TrainingGroup {
            id: "individual:mentee".to_string(),
            name: "Edited".to_string(),
            focus: TrainingFocus::Physical,
            player_ids: vec!["mentee".to_string(), "other".to_string()],
            intensity: None,
            mentor_id: None,
        };

        let merged = merge_training_groups(&team.training_groups, vec![forged]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].focus, TrainingFocus::Technical);
        assert_eq!(merged[0].player_ids, vec!["mentee".to_string()]);
    }

    /// Given a young player nobody has assessed yet (the all-50 default character)
    /// When his first mentoring week runs
    /// Then he holds his assessed character, moved by at most one point a trait, not a jump.
    #[test]
    fn an_unassessed_mentee_is_assessed_before_he_moves() {
        let mentor = strong_mentor("mentor", "team1", "1990-01-01");
        let mut mentee = player("mentee", "team1", "2008-01-01");
        mentee.character = CharacterProfile::default();
        let mut game = game_with(vec![mentor, mentee]);
        let assessed = assessed_character(&game.players[1], game.seed);
        let mut team = game.teams.remove(0);
        plan_individual_development(
            &mut team,
            "mentee",
            "Mentee",
            TrainingFocus::Technical,
            TrainingIntensity::Medium,
            Some("mentor".to_string()),
        )
        .unwrap();
        game.teams.push(team);

        process_weekly_mentoring(&mut game);

        let now = game.players[1].character;
        assert!(!now.is_unassessed());
        assert!(now.professionalism.abs_diff(assessed.professionalism) <= 1);
        assert!(now.ambition.abs_diff(assessed.ambition) <= 1);
        assert!(now.loyalty.abs_diff(assessed.loyalty) <= 1);
    }

    /// Given a senior whose professionalism is ordinary
    /// When his young team-mate is paired with him
    /// Then no week moves the young player: an example worth copying is what transfers
    #[test]
    fn a_weak_mentor_changes_nothing() {
        let mut mentor = strong_mentor("mentor", "team1", "1990-01-01");
        mentor.character.professionalism = 45;
        let mentee = player("mentee", "team1", "2008-01-01");
        let mut game = game_with(vec![mentor, mentee]);
        let mut team = game.teams.remove(0);
        plan_individual_development(
            &mut team,
            "mentee",
            "Mentee",
            TrainingFocus::Technical,
            TrainingIntensity::Medium,
            Some("mentor".to_string()),
        )
        .unwrap();
        game.teams.push(team);

        for _ in 0..12 {
            assert_eq!(process_weekly_mentoring(&mut game), 0);
            game.clock.current_date += chrono::Duration::days(7);
        }

        assert_eq!(game.players[1].character.professionalism, 30);
    }

    fn planned_week() -> DevelopmentWeek {
        DevelopmentWeek {
            focus: TrainingFocus::Technical,
            sessions: 4,
            plan_focus: Some(TrainingFocus::Technical),
            mentor_weeks: 1,
            minutes_played: 90,
            rating_change: 1,
        }
    }

    /// Given six weeks on an individual plan with a mentor and match minutes
    /// When the last six weeks are read, and then the last three
    /// Then the plan and the mentor are the causes, a gain is the effect, the numbers are the
    ///      window's, and every key is a `be.digest` translation key rather than prose
    #[test]
    fn the_digest_reads_the_last_weeks_as_cause_and_effect_keys() {
        let weeks: Vec<DevelopmentWeek> = (0..6).map(|_| planned_week()).collect();

        let digest = development_digest("p-1", &weeks, 6);

        assert_eq!(digest.player_id, "p-1");
        assert_eq!(digest.weeks, 6);
        assert_eq!(digest.net_rating_change, 6);
        assert_eq!(digest.lines.len(), 2);
        let plan = &digest.lines[0];
        assert_eq!(plan.cause_key, CAUSE_INDIVIDUAL_PLAN);
        assert_eq!(plan.effect_key, EFFECT_RATING_GAIN);
        assert_eq!(plan.params["focus"], "Technical");
        assert_eq!(plan.params["weeks"], "6");
        assert_eq!(plan.params["sessions"], "24");
        assert_eq!(plan.params["change"], "6");
        let mentor = &digest.lines[1];
        assert_eq!(mentor.cause_key, CAUSE_MENTOR);
        assert_eq!(mentor.effect_key, EFFECT_CHARACTER_SHIFT);
        assert_eq!(mentor.params["weeks"], "6");
        for line in &digest.lines {
            assert!(line.cause_key.starts_with("be.digest.cause."));
            assert!(line.effect_key.starts_with("be.digest.effect."));
        }

        let shorter = development_digest("p-1", &weeks, 3);
        assert_eq!(shorter.weeks, 3);
        assert_eq!(shorter.net_rating_change, 3);
        assert_eq!(shorter.lines[0].params["sessions"], "12");
    }

    /// Given a player who neither trained nor played for four weeks
    /// When his digest is read
    /// Then it is the idle cause and a stalled effect, not a gain line about nothing
    #[test]
    fn the_digest_holds_an_idle_player_to_a_stalled_line() {
        let idle: Vec<DevelopmentWeek> = (0..4).map(|_| DevelopmentWeek::default()).collect();

        let digest = development_digest("p-idle", &idle, 6);

        assert_eq!(digest.weeks, 4);
        assert_eq!(digest.lines.len(), 1);
        assert_eq!(digest.lines[0].cause_key, CAUSE_IDLE);
        assert_eq!(digest.lines[0].effect_key, EFFECT_STALLED);
        assert_eq!(digest.lines[0].params["weeks"], "4");
        assert!(development_digest("p-none", &[], 6).lines.is_empty());
        assert_eq!(development_digest("p-none", &idle, 0).weeks, 0);
    }

    /// Given a club with an individual plan carrying an intensity and a mentor
    /// When the club is written to a save and read back in a fresh reader
    /// Then the plan, its intensity and its mentor all survive
    #[test]
    fn an_individual_plan_survives_a_save_and_load_round_trip() {
        let mut team = club("team1");
        plan_individual_development(
            &mut team,
            "p-1",
            "Young Player",
            TrainingFocus::Attacking,
            TrainingIntensity::High,
            Some("p-2".to_string()),
        )
        .unwrap();

        let text = serde_json::to_string(&team).unwrap();
        let restored: Team = serde_json::from_str(&text).unwrap();

        let plan = individual_plan_for(&restored, "p-1").unwrap();
        assert_eq!(plan.focus, &TrainingFocus::Attacking);
        assert_eq!(plan.intensity, &TrainingIntensity::High);
        assert_eq!(plan.mentor_id, Some("p-2"));
        assert_eq!(restored.training_groups, team.training_groups);
    }

    /// Given a save written before plans had an intensity or a mentor
    /// When a group from it is read
    /// Then the missing fields read as no intensity and no mentor
    #[test]
    fn a_group_from_an_older_save_reads_without_a_plan() {
        let stored = r#"[{"id":"g1","name":"Defenders","focus":"Defending","player_ids":["p1"]}]"#;

        let groups: Vec<TrainingGroup> = serde_json::from_str(stored).unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].intensity, None);
        assert_eq!(groups[0].mentor_id, None);
        assert!(!is_individual_plan(&groups[0]));
    }

    /// Given a squad with a strong senior, a weak senior, a prime-age player, a young mentee,
    ///       a retired strong senior and a rival club's strong senior
    /// When the club's individual plan options are projected
    /// Then only the active strong senior is listed as a mentor, only the young player as a
    ///      mentee, and the cap matches MAX_INDIVIDUAL_PLANS.
    #[test]
    fn individual_plan_options_filters_mentors_and_mentees_by_club_age_and_character() {
        let mut strong = strong_mentor("mentor-1", "team1", "1992-01-01");
        strong.match_name = "A. Veteran".to_string();
        let mut weak = strong_mentor("weak-senior", "team1", "1990-01-01");
        weak.character.professionalism = 55;
        let prime = strong_mentor("prime", "team1", "2000-01-01");
        let mut young = player("young-1", "team1", "2007-01-01");
        young.match_name = "Y. Prospect".to_string();
        let mut retired = strong_mentor("retired", "team1", "1988-01-01");
        retired.retired = true;
        let rival = strong_mentor("rival-mentor", "team2", "1990-01-01");

        let game = game_with(vec![strong, weak, prime, young, retired, rival]);
        let options = individual_plan_options(&game, "team1");

        assert_eq!(options.max_plans, MAX_INDIVIDUAL_PLANS);
        assert_eq!(
            options.eligible_mentors,
            vec![IndividualPlanCandidate {
                id: "mentor-1".to_string(),
                match_name: "A. Veteran".to_string(),
                age: 34,
            }]
        );
        assert_eq!(
            options.eligible_mentees,
            vec![IndividualPlanCandidate {
                id: "young-1".to_string(),
                match_name: "Y. Prospect".to_string(),
                age: 19,
            }]
        );
    }
}
