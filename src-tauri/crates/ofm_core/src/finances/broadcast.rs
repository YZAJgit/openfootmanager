//! Broadcast income and matchday running costs, settled per completed fixture.
//!
//! The host of a televised match sells its broadcast rights, as it keeps its gate: the share is
//! scaled by the competition, by the division it is played in and by the host's reputation, and
//! the visitors earn nothing from it. The host also pays to stage the match
//! (`upkeep::stadium_matchday_upkeep`). Both are keyed on the
//! fixture the way a gate is (`gate.rs`): the post's id names the fixture and the club, so the
//! journal itself records what has been paid and a fixture is never settled twice.

use std::collections::HashSet;

use chrono::{Duration, NaiveDate};
use domain::finance::CashKind;
use domain::league::{Fixture, FixtureCompetition};

use super::gate::{PAYDAY_INTERVAL_DAYS, completed_fixtures_between, settlement_window_start};
use super::journal::{PostRequest, post_all};
use super::upkeep::stadium_matchday_upkeep;
use crate::game::Game;

const TV_POST_PREFIX: &str = "tv-";
const STADIUM_POST_PREFIX: &str = "stadium-";

/// A top-flight home league match's broadcast share for a club of middling reputation (500), in
/// euros. A season of 19 home matches is about 1.1 million, of the order of a mid-table club's
/// sponsorship over the same weeks.
const TV_BASE_SHARE_PER_MATCH: i64 = 60_000;
/// Each division below the top halves the share, as prize money does; deeper than this the
/// share is no longer halved again (it has become negligible).
const TV_MAX_TIER_HALVINGS: u32 = 6;
const REPUTATION_CEILING: u32 = 1_000;

/// The host's broadcast share for one match, in whole euros: nothing for a friendly, more for
/// continental football than for a domestic cup. `division_tier` is 1 for a top flight and 0 when
/// unknown (read as a top flight).
pub fn tv_share_for(competition: &FixtureCompetition, division_tier: u32, reputation: u32) -> i64 {
    let competition_pct: i64 = match competition {
        FixtureCompetition::League => 100,
        FixtureCompetition::Cup => 40,
        FixtureCompetition::ContinentalClub => 250,
        FixtureCompetition::InternationalClub => 200,
        FixtureCompetition::InternationalNation
        | FixtureCompetition::Friendly
        | FixtureCompetition::FriendlyCup
        | FixtureCompetition::PreseasonTournament => 0,
    };
    // 50% at no reputation, 100% at 500, 150% at the ceiling.
    let reputation_pct = 50 + i64::from(reputation.min(REPUTATION_CEILING)) / 10;
    let halvings = division_tier.saturating_sub(1).min(TV_MAX_TIER_HALVINGS);
    (TV_BASE_SHARE_PER_MATCH * competition_pct / 100 * reputation_pct / 100) >> halvings
}

/// Pay each host its broadcast share, and charge it its staging cost, for every fixture
/// completed in the gate's settlement window that has not been settled yet.
///
/// The window reaches back at most one payday, so a save from before broadcast income existed is
/// not paid retroactively for the weeks it has already played. Each fixture is its own journal
/// batch: one that cannot be posted is retried by the next settlement while still in reach.
pub(super) fn settle_broadcast_and_matchday_costs(game: &mut Game, today: NaiveDate) {
    let from =
        settlement_window_start(game, today).max(today - Duration::days(PAYDAY_INTERVAL_DAYS));
    let settled = settled_post_ids(game);
    let batches: Vec<(String, Vec<PostRequest>)> = completed_fixtures_between(game, from, today)
        .into_iter()
        .map(|fixture| {
            (
                fixture.id.clone(),
                fixture_posts(game, fixture, &settled, today),
            )
        })
        .filter(|(_, reqs)| !reqs.is_empty())
        .collect();
    for (fixture_id, reqs) in batches {
        if let Err(err) = post_all(game, &reqs) {
            log::error!("broadcast settlement for fixture {fixture_id} failed: {err}");
        }
    }
}

fn fixture_posts(
    game: &Game,
    fixture: &Fixture,
    settled: &HashSet<String>,
    today: NaiveDate,
) -> Vec<PostRequest> {
    let tier = division_tier(game, &fixture.competition_id);
    let host_id = &fixture.home_team_id;
    let Some(host) = game.teams.iter().find(|team| &team.id == host_id) else {
        return Vec::new();
    };
    let mut reqs = Vec::new();
    let tv_id = tv_post_id(&fixture.id, host_id);
    let share = tv_share_for(&fixture.competition, tier, host.reputation);
    if share > 0 && !settled.contains(&tv_id) {
        reqs.push(PostRequest::new(host_id, share, CashKind::TvBroadcast, today).with_id(tv_id));
    }
    let stadium_id = stadium_post_id(&fixture.id);
    if !settled.contains(&stadium_id) {
        let cost = stadium_matchday_upkeep(host.stadium_capacity);
        reqs.push(
            PostRequest::new(&fixture.home_team_id, -cost, CashKind::Upkeep, today)
                .with_id(stadium_id),
        );
    }
    reqs
}

/// The division a competition is played in (1 = top flight), 0 when it does not say: a cup, a
/// legacy save, or an edition already archived.
fn division_tier(game: &Game, competition_id: &str) -> u32 {
    game.competitions_in_play()
        .iter()
        .find(|competition| competition.id == competition_id)
        .and_then(|competition| competition.calendar.as_ref())
        .and_then(|calendar| calendar.season.division.as_ref())
        .map_or(0, |division| division.tier)
}

fn settled_post_ids(game: &Game) -> HashSet<String> {
    game.cash_journal
        .iter()
        .filter(|post| {
            post.id.starts_with(TV_POST_PREFIX) || post.id.starts_with(STADIUM_POST_PREFIX)
        })
        .map(|post| post.id.clone())
        .collect()
}

fn tv_post_id(fixture_id: &str, club_id: &str) -> String {
    format!("{TV_POST_PREFIX}{fixture_id}-{club_id}")
}

fn stadium_post_id(fixture_id: &str) -> String {
    format!("{STADIUM_POST_PREFIX}{fixture_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::GameClock;
    use chrono::{TimeZone, Utc};
    use domain::league::{FixtureStatus, League, MatchResult};
    use domain::manager::Manager;
    use domain::team::Team;

    const CAPACITY: u32 = 20_000;

    fn club(id: &str, reputation: u32) -> Team {
        let mut team = Team::new(
            id.to_string(),
            id.to_string(),
            id[..3].to_string(),
            "England".to_string(),
            "Town".to_string(),
            "Ground".to_string(),
            CAPACITY,
        );
        team.reputation = reputation;
        team.finance = 1_000_000;
        team
    }

    fn played(id: &str, date: &str, competition: FixtureCompetition) -> Fixture {
        Fixture {
            id: id.to_string(),
            competition_id: "league".to_string(),
            date: date.to_string(),
            home_team_id: "hosts".to_string(),
            away_team_id: "guests".to_string(),
            competition,
            status: FixtureStatus::Completed,
            result: Some(MatchResult::default()),
            ..Default::default()
        }
    }

    /// Monday 16 June 2025, hosts (reputation 500) against guests (reputation 1,000).
    fn monday_with(fixtures: Vec<Fixture>) -> Game {
        let clock = GameClock::new(Utc.with_ymd_and_hms(2025, 6, 16, 12, 0, 0).unwrap());
        let mut manager = Manager::new(
            "mgr".to_string(),
            "Alex".to_string(),
            "Boss".to_string(),
            "1980-01-01".to_string(),
            "England".to_string(),
        );
        manager.hire("hosts".to_string());
        let mut game = Game::new(
            clock,
            manager,
            vec![club("hosts", 500), club("guests", 1_000)],
            vec![],
            vec![],
            vec![],
        );
        game.competitions = vec![League {
            id: "league".to_string(),
            participant_ids: vec!["hosts".to_string(), "guests".to_string()],
            fixtures,
            ..Default::default()
        }];
        game.sync_legacy_league();
        game
    }

    fn posts_of(game: &Game, club_id: &str, kind: CashKind) -> Vec<i64> {
        game.cash_journal
            .iter()
            .filter(|post| post.club_id == club_id && post.kind == kind)
            .map(|post| post.amount)
            .collect()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2025, 6, 16).unwrap()
    }

    /// Given the base share and the competition, tier and reputation scales
    /// When shares are worked out
    /// Then a top-flight league match pays a middling host the base, a richer one half again,
    ///      a cup less, continental football more, a lower division half per step and a
    ///      friendly nothing.
    #[test]
    fn the_share_scales_with_competition_division_and_reputation() {
        let league = FixtureCompetition::League;
        assert_eq!(tv_share_for(&league, 1, 500), 60_000);
        assert_eq!(tv_share_for(&league, 0, 500), 60_000);
        assert_eq!(tv_share_for(&league, 1, 1_000), 90_000);
        assert_eq!(tv_share_for(&league, 1, 5_000), 90_000);
        assert_eq!(tv_share_for(&league, 2, 500), 30_000);
        assert_eq!(tv_share_for(&league, 3, 500), 15_000);
        assert_eq!(tv_share_for(&FixtureCompetition::Cup, 0, 500), 24_000);
        assert_eq!(
            tv_share_for(&FixtureCompetition::ContinentalClub, 0, 500),
            150_000
        );
        assert_eq!(tv_share_for(&FixtureCompetition::Friendly, 1, 1_000), 0);
    }

    /// Given a league match played on Saturday
    /// When Monday's settlement runs
    /// Then the host is paid its broadcast share and pays a euro a seat to stage it, the
    ///      visitors are paid and charged nothing, and the journal still matches every club's cash.
    #[test]
    fn the_host_sells_the_rights_and_pays_to_stage_the_match() {
        let mut game = monday_with(vec![played("f1", "2025-06-14", FixtureCompetition::League)]);

        settle_broadcast_and_matchday_costs(&mut game, today());

        assert_eq!(
            posts_of(&game, "hosts", CashKind::TvBroadcast),
            vec![60_000]
        );
        assert!(posts_of(&game, "guests", CashKind::TvBroadcast).is_empty());
        assert_eq!(
            posts_of(&game, "hosts", CashKind::Upkeep),
            vec![-i64::from(CAPACITY)]
        );
        assert!(posts_of(&game, "guests", CashKind::Upkeep).is_empty());
        assert!(crate::finances::journal_matches_cash(&game));
    }

    /// Given a match already settled
    /// When the settlement runs again
    /// Then nothing more is posted: the fixture's posts are keyed on it.
    #[test]
    fn a_settled_fixture_is_not_paid_twice() {
        let mut game = monday_with(vec![played("f1", "2025-06-14", FixtureCompetition::League)]);
        settle_broadcast_and_matchday_costs(&mut game, today());
        let posts = game.cash_journal.len();

        settle_broadcast_and_matchday_costs(&mut game, today());

        assert_eq!(game.cash_journal.len(), posts);
    }

    /// Given a match played a month ago that was never settled (a save from before broadcast
    ///       income) and a friendly played on Saturday
    /// When the settlement runs
    /// Then neither earns a broadcast share: the old one is out of reach and a friendly is not
    ///      televised. Staging the friendly still costs the host.
    #[test]
    fn old_matches_and_friendlies_earn_no_share() {
        let mut game = monday_with(vec![
            played("old", "2025-05-17", FixtureCompetition::League),
            played("friendly", "2025-06-14", FixtureCompetition::Friendly),
        ]);
        game.gate_settled_from = Some("2025-05-01".to_string());

        settle_broadcast_and_matchday_costs(&mut game, today());

        assert!(posts_of(&game, "hosts", CashKind::TvBroadcast).is_empty());
        assert!(posts_of(&game, "guests", CashKind::TvBroadcast).is_empty());
        assert_eq!(
            posts_of(&game, "hosts", CashKind::Upkeep),
            vec![-i64::from(CAPACITY)]
        );
    }
}
