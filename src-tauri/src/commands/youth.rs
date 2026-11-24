use crate::commands::util::mutate_active_game_for;
use domain::player::SquadRole;
use domain::team::AcademyTier;
use ofm_core::game::Game;
use ofm_core::state::{Career, StateManager};
use ofm_core::youth_academy::{self, HomeGrownQuota, ScholarshipDecision};
use serde::Serialize;
use std::sync::Arc;
use tauri::State;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
pub struct YouthAcademyOverview {
    pub academy_tier: AcademyTier,
    pub scholars: Vec<ScholarStatusView>,
    pub home_grown: HomeGrownSummaryView,
    pub loaned_youth: Vec<YouthLoanMinutesStatusView>,
}
#[derive(Debug, Clone, Serialize, TS)]
pub struct ScholarStatusView {
    pub player_id: String,
    pub full_name: String,
    pub age: u8,
    pub joined_on: Option<String>,
    pub scholarship_deadline: String,
    pub decision_due: bool,
}
#[derive(Debug, Clone, Serialize, TS)]
pub struct HomeGrownSummaryView {
    pub club_trained_count: u32,
    pub nation_trained_count: u32,
    pub club_trained_quota: u32,
    pub nation_trained_quota: u32,
    pub club_trained_shortfall: u32,
    pub nation_trained_shortfall: u32,
}
#[derive(Debug, Clone, Serialize, TS)]
pub struct YouthLoanMinutesStatusView {
    pub player_id: String,
    pub full_name: String,
    pub loan_team_id: String,
    pub loan_team_name: String,
    pub guaranteed_minutes_pct: Option<u8>,
    pub required_minutes: Option<u32>,
    pub actual_minutes: u32,
    pub available_minutes: Option<u32>,
    pub is_breach: bool,
    pub loan_recallable: bool,
}

#[tauri::command]
pub fn get_youth_academy_overview(
    state: State<'_, Arc<StateManager>>,
) -> Result<Option<YouthAcademyOverview>, String> {
    get_youth_academy_overview_internal(&state)
}
pub fn get_youth_academy_overview_internal(
    state: &StateManager,
) -> Result<Option<YouthAcademyOverview>, String> {
    state
        .get_game(|game| {
            let Some(id) = &game.manager.team_id else {
                return Ok(None);
            };
            let club = game
                .teams
                .iter()
                .find(|team| &team.id == id)
                .ok_or("be.error.managedTeamNotFound")?;
            let today = game.clock.current_date.date_naive();
            let squad: Vec<_> = game
                .players
                .iter()
                .filter(|p| p.team_id.as_ref() == Some(id))
                .cloned()
                .collect();
            let quota = HomeGrownQuota {
                club_trained: 4,
                nation_trained: 8,
            };
            let counts = youth_academy::home_grown_counts(&squad, club, today);
            let club_trained_shortfall = quota.club_trained.saturating_sub(counts.club_trained);
            let nation_trained_shortfall =
                quota.nation_trained.saturating_sub(counts.nation_trained);
            let scholars = squad
                .iter()
                .filter(|p| p.squad_role == SquadRole::Youth)
                .filter_map(|p| {
                    p.academy
                        .scholarship_deadline
                        .as_ref()
                        .map(|deadline| ScholarStatusView {
                            player_id: p.id.clone(),
                            full_name: p.full_name.clone(),
                            age: domain::age::age_on_or_unknown(today, &p.date_of_birth)
                                .clamp(0, 255) as u8,
                            joined_on: p.academy.joined_on.clone(),
                            scholarship_deadline: deadline.clone(),
                            decision_due: today.to_string().as_str() >= deadline.as_str(),
                        })
                })
                .collect();
            let loaned_youth = game
                .players
                .iter()
                .filter(|p| p.squad_role == SquadRole::Youth)
                .filter_map(|p| {
                    let loan = p.active_loan.as_ref().filter(|l| &l.parent_team_id == id)?;
                    let available = youth_academy::available_loan_minutes(game, p);
                    let breach = p.injury.is_none()
                        && available
                            .and_then(|m| youth_academy::guaranteed_minutes_breach(p, m))
                            .is_some();
                    Some(YouthLoanMinutesStatusView {
                        player_id: p.id.clone(),
                        full_name: p.full_name.clone(),
                        loan_team_id: loan.loan_team_id.clone(),
                        loan_team_name: game
                            .teams
                            .iter()
                            .find(|t| t.id == loan.loan_team_id)
                            .map(|t| t.name.clone())
                            .unwrap_or_else(|| loan.loan_team_id.clone()),
                        guaranteed_minutes_pct: p.loan_guaranteed_minutes_pct,
                        required_minutes: available
                            .zip(p.loan_guaranteed_minutes_pct)
                            .map(|(m, pct)| m.saturating_mul(u32::from(pct)).div_ceil(100)),
                        actual_minutes: p
                            .stats
                            .minutes_played
                            .saturating_sub(loan.loan_start_minutes),
                        available_minutes: available,
                        is_breach: breach,
                        loan_recallable: p.loan_recallable,
                    })
                })
                .collect();
            Ok(Some(YouthAcademyOverview {
                academy_tier: club.academy_tier,
                scholars,
                home_grown: HomeGrownSummaryView {
                    club_trained_count: counts.club_trained as u32,
                    nation_trained_count: counts.nation_trained as u32,
                    club_trained_quota: 4,
                    nation_trained_quota: 8,
                    club_trained_shortfall: club_trained_shortfall as u32,
                    nation_trained_shortfall: nation_trained_shortfall as u32,
                },
                loaned_youth,
            }))
        })
        .unwrap_or_else(|| Err("be.error.noActiveGameSession".to_string()))
}
#[tauri::command]
pub fn decide_scholarship_action(
    state: State<'_, Arc<StateManager>>,
    career_token: u64,
    player_id: String,
    offer_pro: bool,
) -> Result<Game, String> {
    decide_scholarship_action_internal(&state, Career::Token(career_token), player_id, offer_pro)
}
pub fn decide_scholarship_action_internal(
    state: &StateManager,
    career: Career,
    player_id: String,
    offer_pro: bool,
) -> Result<Game, String> {
    mutate_active_game_for(state, career, |game| {
        let id = game
            .manager
            .team_id
            .as_ref()
            .ok_or("be.error.noTeamAssigned")?;
        let player = game
            .players
            .iter()
            .find(|p| p.id == player_id)
            .ok_or("be.error.playerNotFound")?;
        if player.team_id.as_ref() != Some(id) {
            return Err("be.error.playerNotFound".to_string());
        }
        youth_academy::decide_scholarship(
            game,
            &player_id,
            if offer_pro {
                ScholarshipDecision::OfferProfessional
            } else {
                ScholarshipDecision::Release
            },
        )
    })
}
#[tauri::command]
pub fn recall_youth_loan_breach(
    state: State<'_, Arc<StateManager>>,
    career_token: u64,
    player_id: String,
) -> Result<Game, String> {
    recall_youth_loan_breach_internal(&state, Career::Token(career_token), player_id)
}
pub fn recall_youth_loan_breach_internal(
    state: &StateManager,
    career: Career,
    player_id: String,
) -> Result<Game, String> {
    mutate_active_game_for(state, career, |game| {
        let id = game
            .manager
            .team_id
            .as_ref()
            .ok_or("be.error.noTeamAssigned")?;
        let player = game
            .players
            .iter()
            .find(|p| p.id == player_id)
            .ok_or("be.error.playerNotFound")?;
        if player
            .active_loan
            .as_ref()
            .is_none_or(|loan| &loan.parent_team_id != id)
        {
            return Err("be.error.notLoanParentClub".to_string());
        }
        youth_academy::recall_youth_loan_for_minutes_breach(game, &player_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use domain::league::{Fixture, FixtureCompetition, FixtureStatus, League};
    use domain::manager::Manager;
    use domain::player::{ActiveLoan, Player, PlayerAttributes, Position, TrainingSpell};
    use domain::team::Team;
    use ofm_core::clock::GameClock;

    fn player(id: &str) -> Player {
        let attributes: PlayerAttributes = serde_json::from_value(serde_json::json!({
            "pace": 60, "stamina": 60, "strength": 60, "agility": 60, "passing": 60,
            "shooting": 60, "tackling": 60, "dribbling": 60, "defending": 60,
            "positioning": 60, "vision": 60, "decisions": 60, "composure": 60,
            "aggression": 60, "teamwork": 60, "leadership": 60,
            "handling": 60, "reflexes": 60, "aerial": 60
        }))
        .unwrap();
        let mut player = Player::new(
            id.into(),
            id.into(),
            id.into(),
            "2008-01-01".into(),
            "ENG".into(),
            Position::Midfielder,
            attributes,
        );
        player.team_id = Some("user".into());
        player.squad_role = SquadRole::Youth;
        player
    }

    fn world() -> Game {
        let mut manager = Manager::new(
            "mgr".into(),
            "Test".into(),
            "Manager".into(),
            "1980-01-01".into(),
            "ENG".into(),
        );
        manager.hire("user".into());
        let club = |id: &str| {
            Team::new(
                id.into(),
                format!("{id} FC"),
                "CLB".into(),
                "ENG".into(),
                "London".into(),
                "Ground".into(),
                20_000,
            )
        };
        let mut game = Game::new(
            GameClock::new(Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap()),
            manager,
            vec![club("user"), club("rival")],
            vec![player("scholar")],
            vec![],
            vec![],
        );
        game.teams[0].academy_tier = AcademyTier::CategoryTwo;
        game.players[0].academy.scholarship_deadline = Some("2026-08-01".into());
        game
    }

    fn state(game: Game) -> StateManager {
        let state = StateManager::new();
        state.set_game(game);
        state
    }

    fn add_loan(game: &mut Game) {
        let mut loanee = player("loanee");
        loanee.team_id = Some("rival".into());
        loanee.loan_guaranteed_minutes_pct = Some(60);
        loanee.loan_recallable = true;
        loanee.stats.minutes_played = 145;
        loanee.active_loan = Some(ActiveLoan {
            parent_team_id: "user".into(),
            loan_team_id: "rival".into(),
            start_date: "2026-07-01".into(),
            end_date: "2027-06-30".into(),
            wage_contribution_pct: 100,
            buy_option_fee: None,
            loan_start_minutes: 100,
            loan_start_appearances: 1,
            development_reported_minutes: 100,
            development_reported_appearances: 1,
        });
        game.players.push(loanee);
        let mut league = League::new(
            "league".into(),
            "League".into(),
            2026,
            &["user".into(), "rival".into()],
        );
        for (index, date) in ["2026-07-10", "2026-07-17", "2026-07-24", "2026-07-31"]
            .iter()
            .enumerate()
        {
            league.fixtures.push(Fixture {
                id: format!("fixture-{index}"),
                competition_id: "league".into(),
                matchday: 1,
                date: (*date).into(),
                home_team_id: "rival".into(),
                away_team_id: "user".into(),
                competition: FixtureCompetition::League,
                status: FixtureStatus::Completed,
                result: None,
                postponement: None,
            });
        }
        game.competitions.push(league);
    }

    #[test]
    fn unemployed_manager_has_no_overview() {
        let mut game = world();
        game.manager.team_id = None;
        assert!(get_youth_academy_overview_internal(&state(game))
            .unwrap()
            .is_none());
    }

    #[test]
    fn overview_reports_scholars_home_grown_and_loans() {
        let mut game = world();
        game.players[0].academy.joined_on = Some("2020-01-01".into());
        game.players[0].academy.training_spells.push(TrainingSpell {
            club_id: "user".into(),
            football_nation: "ENG".into(),
            started_on: "2020-01-01".into(),
            ended_on: Some("2023-01-01".into()),
        });
        let mut future = player("future");
        future.academy.scholarship_deadline = Some("2026-08-02".into());
        game.players.push(future);
        game.players.push(player("no-deadline"));
        let mut senior = player("senior");
        senior.squad_role = SquadRole::Senior;
        senior.academy.scholarship_deadline = Some("2026-08-01".into());
        game.players.push(senior);
        add_loan(&mut game);
        let overview = get_youth_academy_overview_internal(&state(game))
            .unwrap()
            .unwrap();
        assert_eq!(overview.academy_tier, AcademyTier::CategoryTwo);
        let counts = overview.home_grown;
        assert_eq!(
            (counts.club_trained_count, counts.nation_trained_count),
            (1, 1)
        );
        assert_eq!(
            (counts.club_trained_quota, counts.nation_trained_quota),
            (4, 8)
        );
        assert_eq!(
            (
                counts.club_trained_shortfall,
                counts.nation_trained_shortfall
            ),
            (3, 7)
        );
        assert_eq!(overview.scholars.len(), 2);
        let scholar = overview
            .scholars
            .iter()
            .find(|p| p.player_id == "scholar")
            .unwrap();
        assert_eq!(scholar.joined_on.as_deref(), Some("2020-01-01"));
        assert_eq!(scholar.scholarship_deadline, "2026-08-01");
        assert!(scholar.decision_due);
        assert!(
            !overview
                .scholars
                .iter()
                .find(|p| p.player_id == "future")
                .unwrap()
                .decision_due
        );
        assert_eq!(overview.loaned_youth.len(), 1);
        let loan = &overview.loaned_youth[0];
        assert_eq!(
            (&*loan.player_id, &*loan.loan_team_id, &*loan.loan_team_name),
            ("loanee", "rival", "rival FC")
        );
        assert_eq!(
            (
                loan.guaranteed_minutes_pct,
                loan.required_minutes,
                loan.available_minutes
            ),
            (Some(60), Some(216), Some(360))
        );
        assert_eq!(loan.actual_minutes, 45);
        assert!(loan.is_breach && loan.loan_recallable);
    }

    #[test]
    fn scholarship_offer_promotes_and_clears_deadline() {
        let game = decide_scholarship_action_internal(
            &state(world()),
            Career::Active,
            "scholar".into(),
            true,
        )
        .unwrap();
        assert_eq!(game.players[0].squad_role, SquadRole::Senior);
        assert!(game.players[0].academy.scholarship_deadline.is_none());
    }

    #[test]
    fn scholarship_release_removes_club_and_deadline() {
        let game = decide_scholarship_action_internal(
            &state(world()),
            Career::Active,
            "scholar".into(),
            false,
        )
        .unwrap();
        assert!(game.players[0].team_id.is_none());
        assert!(game.players[0].academy.scholarship_deadline.is_none());
    }

    #[test]
    fn scholarship_decision_requires_deadline() {
        let mut game = world();
        game.players[0].academy.scholarship_deadline = None;
        assert_eq!(
            decide_scholarship_action_internal(
                &state(game),
                Career::Active,
                "scholar".into(),
                true
            )
            .err()
            .as_deref(),
            Some("be.error.youthAcademy.noScholarshipDecision")
        );
    }

    #[test]
    fn loan_breach_recall_returns_player() {
        let mut game = world();
        add_loan(&mut game);
        let result =
            recall_youth_loan_breach_internal(&state(game), Career::Active, "loanee".into())
                .unwrap();
        let loanee = result.players.iter().find(|p| p.id == "loanee").unwrap();
        assert_eq!(loanee.team_id.as_deref(), Some("user"));
        assert!(loanee.active_loan.is_none());
    }

    #[test]
    fn loan_recall_rejects_no_breach_or_non_recallable() {
        let mut game = world();
        add_loan(&mut game);
        game.players[1].stats.minutes_played = 400;
        assert_eq!(
            recall_youth_loan_breach_internal(
                &state(game.clone()),
                Career::Active,
                "loanee".into()
            )
            .err()
            .as_deref(),
            Some("be.error.youthAcademy.noMinutesBreach")
        );
        game.players[1].stats.minutes_played = 145;
        game.players[1].loan_recallable = false;
        assert_eq!(
            recall_youth_loan_breach_internal(&state(game), Career::Active, "loanee".into())
                .err()
                .as_deref(),
            Some("be.error.loanNotRecallable")
        );
    }
    #[test]
    fn injured_loanee_is_not_in_breach() {
        let mut game = world();
        add_loan(&mut game);
        game.players[1].injury = Some(domain::player::Injury {
            name: "Injury".into(),
            days_remaining: 5,
        });
        let overview = get_youth_academy_overview_internal(&state(game))
            .unwrap()
            .unwrap();
        assert!(!overview.loaned_youth[0].is_breach);
    }
    #[test]
    fn wrong_club_actions_are_rejected() {
        let mut game = world();
        game.players[0].team_id = Some("rival".into());
        assert_eq!(
            decide_scholarship_action_internal(
                &state(game),
                Career::Active,
                "scholar".into(),
                true
            )
            .err()
            .as_deref(),
            Some("be.error.playerNotFound")
        );
        let mut game = world();
        add_loan(&mut game);
        game.players[1].active_loan.as_mut().unwrap().parent_team_id = "rival".into();
        assert_eq!(
            recall_youth_loan_breach_internal(&state(game), Career::Active, "loanee".into())
                .err()
                .as_deref(),
            Some("be.error.notLoanParentClub")
        );
    }
}
