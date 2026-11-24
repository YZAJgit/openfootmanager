//! M7 youth academy scenarios: academy tiers, regional EPPP compensation,
//! scholarship-to-pro transition, guaranteed loan minutes monitoring, and home-grown rules.

use crate::game::Game;
use chrono::{Datelike, Days, NaiveDate};
use domain::contract_ledger::ContractSource;
use domain::message::{
    ActionOption, ActionType, InboxMessage, MessageAction, MessageCategory, MessageContext,
    MessagePriority,
};
use domain::player::{Player, PlayerMovementKind, SquadRole, TrainingSpell};
use domain::team::{AcademyTier, Team};
use std::collections::HashMap;

const SCHOLARSHIP_AGE: i32 = 18;
const SCHOLARSHIP_DECISION_DAYS: u64 = 30;
const HOME_GROWN_REQUIRED_DAYS: i64 = 3 * 365;
const HOME_GROWN_AGE: u32 = 21;
/// An AI club with this many seniors passes over a scholar, so promotion
/// does not outgrow the squads the population balance is tuned to.
const AI_PROMOTION_SENIOR_CEILING: usize = crate::squad_floor::PLANNING_TARGET_SENIORS;
/// How long an AI club waits before it reconsiders a scholar it passed over.
const SCHOLARSHIP_REVIEW_DAYS: u64 = 365;
const MINUTES_PER_MATCH: u32 = 90;
/// Fewer completed fixtures than this is too small a sample to call a promise broken.
const MIN_FIXTURES_BEFORE_BREACH: usize = 3;

trait AcademyTierRules {
    fn extra_intake(self) -> usize;
    fn quality_delta(self) -> i16;
    fn compensation_basis_points(self) -> u64;
}

impl AcademyTierRules for AcademyTier {
    fn extra_intake(self) -> usize {
        match self {
            Self::CategoryOne => 2,
            Self::CategoryTwo => 1,
            Self::CategoryThree | Self::CategoryFour => 0,
        }
    }

    fn quality_delta(self) -> i16 {
        match self {
            Self::CategoryOne => 6,
            Self::CategoryTwo => 3,
            Self::CategoryThree => 0,
            Self::CategoryFour => -3,
        }
    }

    fn compensation_basis_points(self) -> u64 {
        match self {
            Self::CategoryOne => 400,
            Self::CategoryTwo => 200,
            Self::CategoryThree => 100,
            Self::CategoryFour => 50,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScholarshipDecision {
    #[default]
    OfferProfessional,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HomeGrownQuota {
    pub club_trained: usize,
    pub nation_trained: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HomeGrownShortfall {
    pub club_trained: usize,
    pub nation_trained: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoanMinutesBreach {
    pub player_id: String,
    pub parent_team_id: String,
    pub required_minutes: u32,
    pub actual_minutes: u32,
}

pub fn intake_plan_for_tier<'a>(
    academy: impl IntoIterator<Item = &'a Player>,
    tier: AcademyTier,
) -> crate::youth_intake::IntakePlan {
    let mut plan = crate::youth_intake::plan_with_extra(academy, tier.extra_intake());
    if tier == AcademyTier::CategoryFour {
        plan.groups
            .truncate(plan.groups.len().saturating_sub(1).max(1));
    }
    plan
}

pub(crate) fn apply_academy_tier_to_recruit(recruit: &mut Player, tier: AcademyTier) {
    let delta = tier.quality_delta();
    recruit.ovr = shifted_rating(recruit.ovr, delta);
    recruit.potential = shifted_rating(recruit.potential, delta).max(recruit.ovr);
}

fn shifted_rating(rating: u8, delta: i16) -> u8 {
    (i16::from(rating) + delta).clamp(1, 99) as u8
}

/// EPPP-style training compensation: an annual age-band cost, multiplied by completed academy
/// years and the category's share (Category One 4x, Two 2x, Three 1x, Four 0.5x).
pub fn regional_compensation(age: u32, years_at_academy: u32, tier: AcademyTier) -> u64 {
    let annual: u64 = match age {
        0..=14 => 5_000,
        15..=16 => 15_000,
        17..=18 => 30_000,
        _ => 50_000,
    };
    annual
        .saturating_mul(u64::from(years_at_academy))
        .saturating_mul(tier.compensation_basis_points())
        / 100
}

/// Move an academy player to a rival through the ordinary transfer path, with compensation rather
/// than a negotiated market fee. The selling club receives the transfer income and a keyed event.
pub fn poach_academy_player(
    game: &mut Game,
    player_id: &str,
    rival_team_id: &str,
) -> Result<u64, String> {
    let player = game
        .players
        .iter()
        .find(|player| player.id == player_id)
        .cloned()
        .ok_or("be.error.playerNotFound")?;
    if player.squad_role != SquadRole::Youth {
        return Err("be.error.youthAcademy.notAcademyPlayer".to_string());
    }
    let seller_team_id = player
        .contract_club_id()
        .ok_or("be.error.youthAcademy.notAcademyPlayer")?
        .to_string();
    if seller_team_id == rival_team_id {
        return Err(crate::transfers::ERR_CANNOT_BID_ON_OWN_PLAYER.to_string());
    }
    let seller = game
        .teams
        .iter()
        .find(|team| team.id == seller_team_id)
        .cloned()
        .ok_or("be.error.teamNotFound")?;
    let today = game.clock.current_date.date_naive();
    let age = domain::age::playing_age(today, &player.date_of_birth);
    // A first-year player still cost the club a year of training: never compensate for zero.
    let years = academy_years(&player, today)
        .ok_or("be.error.youthAcademy.notAcademyPlayer")?
        .max(1);
    let fee = regional_compensation(age, years, seller.academy_tier);

    crate::transfers::execute_transfer(game, player_id, rival_team_id, &seller_team_id, fee)?;
    let rival = game
        .teams
        .iter()
        .find(|team| team.id == rival_team_id)
        .cloned()
        .ok_or("be.error.teamNotFound")?;
    if game.manager.team_id.as_deref() == Some(seller_team_id.as_str()) {
        emit_compensation_message(game, &player, &seller, &rival, fee, today);
    }
    Ok(fee)
}

fn academy_years(player: &Player, today: NaiveDate) -> Option<u32> {
    let joined = player
        .academy
        .joined_on
        .as_deref()
        .or(player.contract_start())?;
    let joined = NaiveDate::parse_from_str(joined, "%Y-%m-%d").ok()?;
    Some(domain::age::age_between(joined, today).max(0) as u32)
}

pub(crate) fn record_academy_move(player: &mut Player, club: &Team, date: NaiveDate) {
    let today = crate::contracts::iso(date);
    if let Some(spell) = player
        .academy
        .training_spells
        .iter_mut()
        .find(|spell| spell.ended_on.is_none())
    {
        spell.ended_on = Some(today.clone());
    }
    player.academy.joined_on = Some(today.clone());
    player.academy.scholarship_deadline = None;
    player.academy.training_spells.push(TrainingSpell {
        club_id: club.id.clone(),
        football_nation: club.football_nation.clone(),
        started_on: today,
        ended_on: None,
    });
}

/// Ends the spell he is training under, on the day he leaves the club (or retires).
pub(crate) fn end_academy_spell(player: &mut Player, date: NaiveDate) {
    let today = crate::contracts::iso(date);
    for spell in player
        .academy
        .training_spells
        .iter_mut()
        .filter(|spell| spell.ended_on.is_none())
    {
        spell.ended_on = Some(today.clone());
    }
}

/// Opens a training spell when an under-21 joins a club: only the years before twenty-one count
/// toward the home-grown rules, so an older arrival has nothing to record.
pub(crate) fn begin_academy_spell(player: &mut Player, club: &Team, date: NaiveDate) {
    if domain::age::playing_age(date, &player.date_of_birth) < HOME_GROWN_AGE {
        record_academy_move(player, club, date);
    }
}

fn emit_compensation_message(
    game: &mut Game,
    player: &Player,
    seller: &Team,
    rival: &Team,
    fee: u64,
    date: NaiveDate,
) {
    let mut params = HashMap::new();
    params.insert("player".to_string(), player.full_name.clone());
    params.insert("fromTeam".to_string(), seller.name.clone());
    params.insert("toTeam".to_string(), rival.name.clone());
    params.insert("fee".to_string(), fee.to_string());
    let message = base_message(
        format!("youth_compensation_{}_{}", player.id, rival.id),
        "be.msg.youthCompensation.subject",
        "be.msg.youthCompensation.body",
        params,
        date,
    )
    .with_category(MessageCategory::Transfer)
    .with_context(MessageContext {
        team_id: Some(seller.id.clone()),
        team_name: Some(seller.name.clone()),
        player_id: Some(player.id.clone()),
        ..MessageContext::default()
    });
    crate::inbox::emit(game, message);
}

/// Open the scholar decision at eighteen, then enforce it on the deadline. AI clubs promote a
/// scholar whose potential is at least 55 while the club has room and keep the rest on their academy
/// contract; an unanswered user decision releases.
pub fn process_scholarship_transitions(game: &mut Game) {
    let today = game.clock.current_date.date_naive();
    let scholars: Vec<String> = game
        .players
        .iter()
        .filter(|player| {
            player.squad_role == SquadRole::Youth
                && player.contract_club_id().is_some()
                && domain::age::age_on_or_unknown(today, &player.date_of_birth) >= SCHOLARSHIP_AGE
        })
        .map(|player| player.id.clone())
        .collect();
    for player_id in scholars {
        open_scholarship_deadline(game, &player_id, today);
        let Some(deadline) = game
            .players
            .iter()
            .find(|player| player.id == player_id)
            .and_then(|player| player.academy.scholarship_deadline.clone())
        else {
            continue;
        };
        if deadline <= crate::contracts::iso(today) {
            decide_due_scholarship(game, &player_id);
        }
    }
}

fn open_scholarship_deadline(game: &mut Game, player_id: &str, today: NaiveDate) {
    let deadline = today + Days::new(SCHOLARSHIP_DECISION_DAYS);
    let deadline_text = crate::contracts::iso(deadline);
    let Some(player) = game
        .players
        .iter_mut()
        .find(|player| player.id == player_id)
    else {
        return;
    };
    if player.academy.scholarship_deadline.is_some() {
        return;
    }
    player.academy.scholarship_deadline = Some(deadline_text.clone());
    let player = player.clone();
    if game.manager.team_id.as_deref() == player.contract_club_id() {
        emit_scholarship_message(game, &player, &deadline_text, today);
    }
}

fn decide_due_scholarship(game: &mut Game, player_id: &str) {
    let decision = game
        .players
        .iter()
        .find(|player| player.id == player_id)
        .and_then(|player| {
            if game.manager.team_id.as_deref() == player.contract_club_id() {
                return Some(ScholarshipDecision::Release);
            }
            // An AI club only ever promotes: a scholar it passes over stays on the academy
            // contract it already has, so the free-agent market is not flooded at eighteen.
            let club_has_room = player.contract_club_id().is_some_and(|club| {
                crate::squad_floor::senior_counts(game, club)
                    .iter()
                    .sum::<usize>()
                    < AI_PROMOTION_SENIOR_CEILING
            });
            (player.potential >= 55 && club_has_room)
                .then_some(ScholarshipDecision::OfferProfessional)
        });
    let promoted = decision.map(|decision| decide_scholarship(game, player_id, decision));
    match promoted {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            log::info!("[youth_academy] scholar {player_id} not promoted: {error}");
            defer_scholarship_decision(game, player_id);
        }
        None => defer_scholarship_decision(game, player_id),
    }
}

/// An AI club that passes over a scholar asks again next year: a deadline left in the past would
/// rerun the whole decision, and its squad count, every day.
fn defer_scholarship_decision(game: &mut Game, player_id: &str) {
    let next_review = crate::contracts::iso(
        game.clock.current_date.date_naive() + Days::new(SCHOLARSHIP_REVIEW_DAYS),
    );
    if let Some(player) = game
        .players
        .iter_mut()
        .find(|player| player.id == player_id)
        .filter(|player| player.squad_role == SquadRole::Youth)
    {
        player.academy.scholarship_deadline = Some(next_review);
    }
}

pub fn decide_scholarship(
    game: &mut Game,
    player_id: &str,
    decision: ScholarshipDecision,
) -> Result<(), String> {
    let today = game.clock.current_date.date_naive();
    let player_index = game
        .players
        .iter()
        .position(|player| player.id == player_id)
        .ok_or("be.error.playerNotFound")?;
    let player = &game.players[player_index];
    if player.squad_role != SquadRole::Youth
        || domain::age::age_on_or_unknown(today, &player.date_of_birth) < SCHOLARSHIP_AGE
        || player.academy.scholarship_deadline.is_none()
    {
        return Err("be.error.youthAcademy.noScholarshipDecision".to_string());
    }
    match decision {
        ScholarshipDecision::OfferProfessional => offer_professional_contract(game, player_index),
        ScholarshipDecision::Release => {
            crate::contracts::release_player_contract(
                game,
                player_index,
                crate::contracts::ContractReleaseReason::ManagerTermination { severance_cost: 0 },
            );
            game.players[player_index].academy.scholarship_deadline = None;
            Ok(())
        }
    }
}

/// Applies the manager's answer to a scholarship decision mail. `None` when the action is not a
/// scholarship decision, so the caller falls through to the other response handlers.
pub fn apply_scholarship_response(
    game: &mut Game,
    message_id: &str,
    action_id: &str,
    option_id: &str,
) -> Option<Result<(), String>> {
    let player_id = action_id.strip_prefix("scholarship_decision_")?;
    let decision = if option_id == format!("scholarship_pro_{player_id}") {
        ScholarshipDecision::OfferProfessional
    } else if option_id == format!("scholarship_release_{player_id}") {
        ScholarshipDecision::Release
    } else {
        return None;
    };
    let player_id = player_id.to_string();
    if let Err(error) = decide_scholarship(game, &player_id, decision) {
        return Some(Err(error));
    }
    if let Some(action) = game
        .messages
        .iter_mut()
        .find(|message| message.id == message_id)
        .and_then(|message| {
            message
                .actions
                .iter_mut()
                .find(|action| action.id == action_id)
        })
    {
        action.resolved = true;
    }
    Some(Ok(()))
}

fn offer_professional_contract(game: &mut Game, player_index: usize) -> Result<(), String> {
    let today = game.clock.current_date.date_naive();
    let player = game.players[player_index].clone();
    let club_id = player
        .contract_club_id()
        .ok_or("be.error.youthAcademy.noScholarshipDecision")?;
    let club = game
        .teams
        .iter()
        .find(|team| team.id == club_id)
        .cloned()
        .ok_or("be.error.teamNotFound")?;
    let (wage, end) = crate::contracts::standard_contract_terms(&player, &club, today, 0)
        .ok_or(crate::contracts::ERR_UNABLE_TO_CALCULATE_CONTRACT_END_DATE)?;
    if !crate::contract_wage_policy::wage_policy_verdict(game, &club, &player, wage).permits() {
        return Err(crate::contract_wage_policy::renewal_wage_policy_error_message(&club));
    }
    let player = &mut game.players[player_index];
    crate::contracts::record_movement(
        player,
        crate::contracts::contract_entry(
            PlayerMovementKind::Renewal,
            today,
            &club,
            crate::contracts::contract_record(today, end, wage, ContractSource::Renewal),
        ),
    );
    player.squad_role = SquadRole::Senior;
    player.academy.scholarship_deadline = None;
    Ok(())
}

fn emit_scholarship_message(game: &mut Game, player: &Player, deadline: &str, today: NaiveDate) {
    let mut params = HashMap::new();
    params.insert("player".to_string(), player.full_name.clone());
    params.insert("deadline".to_string(), deadline.to_string());
    let options = vec![
        ActionOption {
            id: format!("scholarship_pro_{}", player.id),
            label: String::new(),
            description: String::new(),
            label_key: Some("be.msg.scholarshipDecision.offer".to_string()),
            description_key: Some("be.msg.scholarshipDecision.offerDescription".to_string()),
        },
        ActionOption {
            id: format!("scholarship_release_{}", player.id),
            label: String::new(),
            description: String::new(),
            label_key: Some("be.msg.scholarshipDecision.release".to_string()),
            description_key: Some("be.msg.scholarshipDecision.releaseDescription".to_string()),
        },
    ];
    let message = base_message(
        format!("scholarship_decision_{}", player.id),
        "be.msg.scholarshipDecision.subject",
        "be.msg.scholarshipDecision.body",
        params,
        today,
    )
    .with_category(MessageCategory::Contract)
    .with_priority(MessagePriority::High)
    .with_action(MessageAction {
        id: format!("scholarship_decision_{}", player.id),
        label: String::new(),
        action_type: ActionType::ChooseOption { options },
        resolved: false,
        label_key: None,
    })
    .with_context(MessageContext {
        team_id: player.contract_club_id().map(str::to_string),
        player_id: Some(player.id.clone()),
        ..MessageContext::default()
    });
    crate::inbox::emit(game, message);
}

pub fn home_grown_shortfall(
    squad: &[Player],
    club: &Team,
    quota: HomeGrownQuota,
    on: NaiveDate,
) -> HomeGrownShortfall {
    let counts = home_grown_counts(squad, club, on);
    HomeGrownShortfall {
        club_trained: quota.club_trained.saturating_sub(counts.club_trained),
        nation_trained: quota.nation_trained.saturating_sub(counts.nation_trained),
    }
}

pub fn home_grown_counts(squad: &[Player], club: &Team, on: NaiveDate) -> HomeGrownQuota {
    let club_trained = squad
        .iter()
        .filter(|player| is_home_grown(player, on, |spell| spell.club_id == club.id))
        .count();
    let nation_trained = squad
        .iter()
        .filter(|player| {
            is_home_grown(player, on, |spell| {
                spell.football_nation == club.football_nation
            })
        })
        .count();
    HomeGrownQuota {
        club_trained,
        nation_trained,
    }
}

fn is_home_grown(
    player: &Player,
    on: NaiveDate,
    relevant: impl Fn(&TrainingSpell) -> bool + Copy,
) -> bool {
    training_days_before_21(player, on, relevant) >= HOME_GROWN_REQUIRED_DAYS
}

fn training_days_before_21(
    player: &Player,
    on: NaiveDate,
    relevant: impl Fn(&TrainingSpell) -> bool,
) -> i64 {
    let Some(born) = NaiveDate::parse_from_str(&player.date_of_birth, "%Y-%m-%d").ok() else {
        return 0;
    };
    let Some(twenty_first) = twenty_first_birthday(born) else {
        return 0;
    };
    let cutoff = on.min(twenty_first);
    let mut spells: Vec<(NaiveDate, NaiveDate)> = player
        .academy
        .training_spells
        .iter()
        .filter(|spell| relevant(spell))
        .filter_map(|spell| clipped_training_spell(spell, on, cutoff))
        .collect();
    spells.sort_unstable();
    merged_days(spells)
}

/// The day he turns twenty-one by the project's age rule: a 29 February birthday falls on 1 March
/// in a common year (`domain::age::age_between`), not on 28 February.
fn twenty_first_birthday(born: NaiveDate) -> Option<NaiveDate> {
    let year = born.year() + HOME_GROWN_AGE as i32;
    NaiveDate::from_ymd_opt(year, born.month(), born.day())
        .or_else(|| NaiveDate::from_ymd_opt(year, 3, 1))
}

fn clipped_training_spell(
    spell: &TrainingSpell,
    on: NaiveDate,
    cutoff: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let start = NaiveDate::parse_from_str(&spell.started_on, "%Y-%m-%d").ok()?;
    let end = spell
        .ended_on
        .as_deref()
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .unwrap_or(on)
        .min(cutoff);
    (start < end && start < cutoff).then_some((start, end))
}

fn merged_days(mut spells: Vec<(NaiveDate, NaiveDate)>) -> i64 {
    let Some(mut current) = spells.first().copied() else {
        return 0;
    };
    let mut days = 0;
    for (start, end) in spells.drain(1..) {
        if start <= current.1 {
            current.1 = current.1.max(end);
        } else {
            days += (current.1 - current.0).num_days();
            current = (start, end);
        }
    }
    days + (current.1 - current.0).num_days()
}

pub fn guaranteed_minutes_breach(
    player: &Player,
    available_minutes: u32,
) -> Option<LoanMinutesBreach> {
    if player.squad_role != SquadRole::Youth {
        return None;
    }
    let loan = player.active_loan.as_ref()?;
    let guaranteed_pct = u32::from(player.loan_guaranteed_minutes_pct?);
    let required_minutes = (available_minutes.saturating_mul(guaranteed_pct)).div_ceil(100);
    let actual_minutes = player
        .stats
        .minutes_played
        .saturating_sub(loan.loan_start_minutes);
    (actual_minutes < required_minutes).then(|| LoanMinutesBreach {
        player_id: player.id.clone(),
        parent_team_id: loan.parent_team_id.clone(),
        required_minutes,
        actual_minutes,
    })
}

pub fn monitor_guaranteed_youth_loan_minutes(game: &mut Game) -> Vec<LoanMinutesBreach> {
    let loanees: Vec<Player> = game
        .players
        .iter()
        .filter(|player| player.squad_role == SquadRole::Youth && player.active_loan.is_some())
        .cloned()
        .collect();
    let mut breaches = Vec::new();
    for player in loanees {
        if player.injury.is_some() {
            continue;
        }
        let Some(available) = available_loan_minutes(game, &player) else {
            continue;
        };
        if let Some(breach) = guaranteed_minutes_breach(&player, available) {
            if game.manager.team_id.as_deref() == Some(breach.parent_team_id.as_str()) {
                emit_loan_breach_message(game, &player, &breach);
            }
            breaches.push(breach);
        }
    }
    breaches
}

/// Minutes the loan club has played since the loan began, or `None` while too few fixtures are
/// complete to judge. Fixtures dated on the start day are skipped: matches are simulated before
/// the day's loan registrations, so such a fixture was played before the loan existed.
pub fn available_loan_minutes(game: &Game, player: &Player) -> Option<u32> {
    let loan = player.active_loan.as_ref()?;
    let today = crate::contracts::iso(game.clock.current_date.date_naive());
    let matches = game
        .competitions_in_play()
        .iter()
        .flat_map(|competition| competition.fixtures.iter())
        .filter(|fixture| {
            fixture.status == domain::league::FixtureStatus::Completed
                && fixture.date > loan.start_date
                && fixture.date <= today
                && (fixture.home_team_id == loan.loan_team_id
                    || fixture.away_team_id == loan.loan_team_id)
        })
        .count();
    (matches >= MIN_FIXTURES_BEFORE_BREACH).then(|| {
        u32::try_from(matches)
            .unwrap_or(u32::MAX)
            .saturating_mul(MINUTES_PER_MATCH)
    })
}

fn emit_loan_breach_message(game: &mut Game, player: &Player, breach: &LoanMinutesBreach) {
    let today = game.clock.current_date.date_naive();
    let week = today.iso_week();
    let mut params = HashMap::new();
    params.insert("player".to_string(), player.full_name.clone());
    params.insert("actual".to_string(), breach.actual_minutes.to_string());
    params.insert("required".to_string(), breach.required_minutes.to_string());
    let message = base_message(
        format!(
            "youth_loan_minutes_{}_{}_w{:02}",
            player.id,
            week.year(),
            week.week()
        ),
        "be.msg.youthLoanMinutesBreach.subject",
        "be.msg.youthLoanMinutesBreach.body",
        params,
        today,
    )
    .with_category(MessageCategory::PlayerMorale)
    .with_priority(MessagePriority::High)
    .with_context(MessageContext {
        team_id: Some(breach.parent_team_id.clone()),
        player_id: Some(player.id.clone()),
        ..MessageContext::default()
    });
    crate::inbox::emit(game, message);
}

pub fn recall_youth_loan_for_minutes_breach(
    game: &mut Game,
    player_id: &str,
) -> Result<(), String> {
    let player = game
        .players
        .iter()
        .find(|player| player.id == player_id)
        .ok_or("be.error.playerNotFound")?;
    let available_minutes =
        available_loan_minutes(game, player).ok_or("be.error.youthAcademy.noMinutesBreach")?;
    let breach = guaranteed_minutes_breach(player, available_minutes)
        .filter(|_| player.injury.is_none())
        .ok_or("be.error.youthAcademy.noMinutesBreach")?;
    crate::transfers::recall_loan(game, player_id, &breach.parent_team_id).map_err(|error| {
        match error {
            crate::transfers::LoanRecallRefusal::PlayerNotFound => "be.error.playerNotFound",
            crate::transfers::LoanRecallRefusal::NotOnLoan => "be.error.playerNotOnLoan",
            crate::transfers::LoanRecallRefusal::NotParentClub => "be.error.notLoanParentClub",
            crate::transfers::LoanRecallRefusal::NotRecallable => "be.error.loanNotRecallable",
        }
        .to_string()
    })
}

fn base_message(
    id: String,
    subject_key: &str,
    body_key: &str,
    params: HashMap<String, String>,
    date: NaiveDate,
) -> InboxMessage {
    InboxMessage::new(
        id,
        String::new(),
        String::new(),
        String::new(),
        crate::contracts::iso(date),
    )
    .with_sender_role("")
    .with_i18n(subject_key, body_key, params)
    .with_sender_i18n("be.sender.assistantManager", "be.role.assistantManager")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::GameClock;
    use crate::test_support::uniform_attributes;
    use chrono::{TimeZone, Utc};
    use domain::league::{Fixture, FixtureCompetition, FixtureStatus, League};
    use domain::manager::Manager;
    use domain::player::{ActiveLoan, Position, TrainingSpell};
    use domain::team::AcademyTier;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn club(id: &str) -> Team {
        let mut team = Team::new(
            id.to_string(),
            format!("{id} FC"),
            "CLB".to_string(),
            "ENG".to_string(),
            "London".to_string(),
            "Ground".to_string(),
            20_000,
        );
        team.finance = 10_000_000;
        team.transfer_budget = 10_000_000;
        team.wage_budget = 10_000_000;
        team
    }

    fn player(id: &str, club_id: &str, born: &str) -> Player {
        let mut player = Player::new(
            id.to_string(),
            id.to_string(),
            id.to_string(),
            born.to_string(),
            "ENG".to_string(),
            Position::Midfielder,
            uniform_attributes(50),
        );
        player.team_id = Some(club_id.to_string());
        player.squad_role = SquadRole::Youth;
        player.ovr = 50;
        player.potential = 70;
        player
    }

    fn world() -> Game {
        let clock = GameClock::new(Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap());
        let mut manager = Manager::new(
            "mgr".to_string(),
            "Test".to_string(),
            "Manager".to_string(),
            "1980-01-01".to_string(),
            "England".to_string(),
        );
        manager.hire("user".to_string());
        let scholar = player("scholar", "user", "2008-08-01");
        Game::new(
            clock,
            manager,
            vec![club("user"), club("rival")],
            vec![scholar],
            vec![],
            vec![],
        )
    }

    /// Given academies in different categories and an identical recruit
    /// When intake size and recruit quality are adjusted for the category
    /// Then Category One produces five higher-quality recruits and Category Four only two.
    #[test]
    fn academy_tiers_change_intake_count_and_quality() {
        let empty: Vec<Player> = Vec::new();
        assert_eq!(
            intake_plan_for_tier(&empty, AcademyTier::CategoryOne)
                .groups
                .len(),
            5
        );
        assert_eq!(
            intake_plan_for_tier(&empty, AcademyTier::CategoryFour)
                .groups
                .len(),
            2
        );

        let mut recruit = player("recruit", "user", "2010-01-01");
        apply_academy_tier_to_recruit(&mut recruit, AcademyTier::CategoryOne);
        assert_eq!((recruit.ovr, recruit.potential), (56, 76));
    }

    /// Given a 17-year-old with two years at a Category Two academy and an AI rival
    /// When the rival poaches him
    /// Then his club receives 120,000 euros and a compensation inbox event.
    #[test]
    fn an_ai_poach_pays_regional_compensation_to_the_academy() {
        let mut game = world();
        game.teams[0].academy_tier = AcademyTier::CategoryTwo;
        game.players[0].date_of_birth = "2009-08-01".to_string();
        game.players[0].academy.joined_on = Some("2024-08-01".to_string());
        let seller_cash = game.teams[0].finance;

        let fee = poach_academy_player(&mut game, "scholar", "rival").unwrap();

        assert_eq!(fee, 120_000);
        assert_eq!(game.players[0].team_id.as_deref(), Some("rival"));
        assert_eq!(game.teams[0].finance, seller_cash + 120_000);
        assert!(game.messages.iter().any(|message| {
            message.subject_key.as_deref() == Some("be.msg.youthCompensation.subject")
                && message.i18n_params.get("fee").map(String::as_str) == Some("120000")
        }));
    }

    /// Given a scholar on his eighteenth birthday
    /// When transitions run and the manager offers a professional contract
    /// Then a deadline and keyed decision arrive, and accepting promotes him on a new contract.
    #[test]
    fn a_scholar_gets_a_deadline_and_can_turn_professional() {
        let mut game = world();
        process_scholarship_transitions(&mut game);

        assert_eq!(
            game.players[0].academy.scholarship_deadline.as_deref(),
            Some("2026-08-31")
        );
        assert!(game.messages.iter().any(|message| {
            message.subject_key.as_deref() == Some("be.msg.scholarshipDecision.subject")
                && message.i18n_params.get("deadline").map(String::as_str) == Some("2026-08-31")
        }));

        decide_scholarship(&mut game, "scholar", ScholarshipDecision::OfferProfessional).unwrap();
        assert_eq!(game.players[0].squad_role, SquadRole::Senior);
        assert_eq!(game.players[0].contract_start(), Some("2026-08-01"));
        assert_eq!(game.players[0].academy.scholarship_deadline, None);
    }

    /// Given one player with three pre-21 years at the club and another with only two national years
    /// When a squad requiring one club-trained and two nation-trained players is checked
    /// Then only the nation-trained quota is short by one.
    #[test]
    fn home_grown_checker_returns_the_registration_shortfall() {
        let mut trained = player("trained", "user", "2000-01-01");
        trained.academy.training_spells.push(TrainingSpell {
            club_id: "user".to_string(),
            football_nation: "ENG".to_string(),
            started_on: "2018-01-01".to_string(),
            ended_on: Some("2021-01-01".to_string()),
        });
        let mut short = player("short", "rival", "2000-01-01");
        short.academy.training_spells.push(TrainingSpell {
            club_id: "rival".to_string(),
            football_nation: "ENG".to_string(),
            started_on: "2019-01-01".to_string(),
            ended_on: Some("2021-01-01".to_string()),
        });

        let squad = [trained, short];
        assert_eq!(
            home_grown_counts(&squad, &club("user"), day(2022, 1, 1)),
            HomeGrownQuota {
                club_trained: 1,
                nation_trained: 1
            }
        );
        let shortfall = home_grown_shortfall(
            &squad,
            &club("user"),
            HomeGrownQuota {
                club_trained: 1,
                nation_trained: 2,
            },
            day(2022, 1, 1),
        );

        assert_eq!(
            shortfall,
            HomeGrownShortfall {
                club_trained: 0,
                nation_trained: 1,
            }
        );
    }

    fn loan_fixture(id: &str, date: &str) -> Fixture {
        Fixture {
            id: id.to_string(),
            competition_id: "league".to_string(),
            matchday: 1,
            date: date.to_string(),
            home_team_id: "rival".to_string(),
            away_team_id: "user".to_string(),
            competition: FixtureCompetition::League,
            status: FixtureStatus::Completed,
            result: None,
            postponement: None,
        }
    }

    /// A promised-60-percent loanee at "rival" who has played 45 minutes of the loan, with the
    /// loan club's completed fixtures on the given dates.
    fn loan_world(fixture_dates: &[&str]) -> Game {
        let mut game = world();
        let loanee = &mut game.players[0];
        loanee.loan_guaranteed_minutes_pct = Some(60);
        loanee.loan_recallable = true;
        loanee.team_id = Some("rival".to_string());
        loanee.stats.minutes_played = 145;
        loanee.active_loan = Some(ActiveLoan {
            parent_team_id: "user".to_string(),
            loan_team_id: "rival".to_string(),
            start_date: "2026-07-01".to_string(),
            end_date: "2027-06-30".to_string(),
            wage_contribution_pct: 100,
            buy_option_fee: None,
            loan_start_minutes: 100,
            loan_start_appearances: 1,
            development_reported_minutes: 100,
            development_reported_appearances: 1,
        });
        let mut league = League::new(
            "league".to_string(),
            "League".to_string(),
            2026,
            &["user".to_string(), "rival".to_string()],
        );
        for (index, date) in fixture_dates.iter().enumerate() {
            league
                .fixtures
                .push(loan_fixture(&format!("fixture-{index}"), date));
        }
        game.competitions.push(league);
        game
    }

    const FOUR_FIXTURES: [&str; 4] = ["2026-07-10", "2026-07-17", "2026-07-24", "2026-07-31"];

    /// Given a youth loanee promised 60 percent of 360 available minutes but given only 45
    /// When the weekly monitor runs and his parent recalls him
    /// Then the shortfall is reported by inbox key and the existing recall ends the loan.
    #[test]
    fn a_guaranteed_minutes_breach_is_reported_and_permits_recall() {
        let mut game = loan_world(&FOUR_FIXTURES);

        let breaches = monitor_guaranteed_youth_loan_minutes(&mut game);

        assert_eq!(breaches.len(), 1);
        assert_eq!(breaches[0].required_minutes, 216);
        assert_eq!(breaches[0].actual_minutes, 45);
        assert!(game.messages.iter().any(|message| {
            message.body_key.as_deref() == Some("be.msg.youthLoanMinutesBreach.body")
        }));
        recall_youth_loan_for_minutes_breach(&mut game, "scholar").unwrap();
        assert_eq!(game.players[0].team_id.as_deref(), Some("user"));
        assert!(game.players[0].active_loan.is_none());
    }

    /// Given a loanee whose club played on the day the loan began and only twice since
    /// When the monitor runs
    /// Then the start-day fixture is not counted and two fixtures are too few to flag a breach.
    #[test]
    fn a_loan_is_not_flagged_on_the_start_day_fixture_or_a_tiny_sample() {
        let mut game = loan_world(&["2026-07-01", "2026-07-10", "2026-07-17"]);

        assert!(monitor_guaranteed_youth_loan_minutes(&mut game).is_empty());
        assert_eq!(
            recall_youth_loan_for_minutes_breach(&mut game, "scholar"),
            Err("be.error.youthAcademy.noMinutesBreach".to_string())
        );
    }

    /// Given an injured loanee who has missed enough fixtures to look like a breach
    /// When the monitor runs
    /// Then he is not flagged, because the loan club could not have played him.
    #[test]
    fn an_injured_loanee_is_not_flagged() {
        let mut game = loan_world(&FOUR_FIXTURES);
        game.players[0].injury = Some(domain::player::Injury {
            name: "Sprain".to_string(),
            days_remaining: 10,
        });

        assert!(monitor_guaranteed_youth_loan_minutes(&mut game).is_empty());
    }

    /// Given a caller that cannot know the real fixture count
    /// When a recall is asked for a loanee the loan club did play
    /// Then the recall computes the minutes itself and refuses.
    #[test]
    fn a_recall_is_refused_when_the_loanee_has_had_his_minutes() {
        let mut game = loan_world(&FOUR_FIXTURES);
        game.players[0].stats.minutes_played = 100 + 300;

        assert_eq!(
            recall_youth_loan_for_minutes_breach(&mut game, "scholar"),
            Err("be.error.youthAcademy.noMinutesBreach".to_string())
        );
    }

    /// Given a scholar who joined the academy a month before the club releases him
    /// When the club releases him
    /// Then his training spell is closed that day and he does not count as home-grown.
    #[test]
    fn a_released_scholar_closes_his_spell_and_is_not_home_grown() {
        let mut game = world();
        let user = game.teams[0].clone();
        begin_academy_spell(&mut game.players[0], &user, day(2026, 7, 1));
        process_scholarship_transitions(&mut game);

        decide_scholarship(&mut game, "scholar", ScholarshipDecision::Release).unwrap();

        let spells = &game.players[0].academy.training_spells;
        assert_eq!(spells.len(), 1);
        assert_eq!(spells[0].ended_on.as_deref(), Some("2026-08-01"));
        let shortfall = home_grown_shortfall(
            &game.players,
            &user,
            HomeGrownQuota {
                club_trained: 1,
                nation_trained: 1,
            },
            day(2030, 1, 1),
        );
        assert_eq!(shortfall.club_trained, 1);
    }

    /// Given a player born on 29 February
    /// When his twenty-first birthday is reckoned in a common year
    /// Then it is 1 March, matching `domain::age`, so no training day is lost.
    #[test]
    fn a_leap_day_birthday_turns_twenty_one_on_the_first_of_march() {
        assert_eq!(
            twenty_first_birthday(day(2004, 2, 29)),
            Some(day(2025, 3, 1))
        );
        assert_eq!(
            twenty_first_birthday(day(2000, 2, 29)),
            Some(day(2021, 3, 1))
        );
        assert_eq!(
            twenty_first_birthday(day(2000, 5, 4)),
            Some(day(2021, 5, 4))
        );
    }

    /// Given an open scholarship decision mail for the user's scholar
    /// When the manager's answer arrives through the response handler
    /// Then it applies his choice and resolves the action; a foreign option or action is not its.
    #[test]
    fn the_manager_answer_to_a_scholarship_mail_is_applied() {
        let mut game = world();
        process_scholarship_transitions(&mut game);
        let id = "scholarship_decision_scholar";

        assert!(
            apply_scholarship_response(&mut game, id, "other", "scholarship_pro_scholar").is_none()
        );
        assert!(apply_scholarship_response(&mut game, id, id, "discard").is_none());
        assert_eq!(
            apply_scholarship_response(&mut game, id, id, "scholarship_pro_scholar"),
            Some(Ok(()))
        );

        assert_eq!(game.players[0].squad_role, SquadRole::Senior);
        let message = game
            .messages
            .iter()
            .find(|message| message.id == id)
            .unwrap();
        assert!(message.actions[0].resolved);
    }

    /// Given an empty Category One academy and a full one
    /// When the tiered intake is planned
    /// Then the extras spread over positions rather than repeating one, and a full academy is
    /// not pushed past its capacity.
    #[test]
    fn extra_intake_spreads_over_positions_and_respects_capacity() {
        let empty: Vec<Player> = Vec::new();
        let plan = intake_plan_for_tier(&empty, AcademyTier::CategoryOne);
        let keepers = plan
            .groups
            .iter()
            .filter(|position| **position == Position::Goalkeeper)
            .count();
        assert_eq!(plan.groups.len(), 5);
        assert_eq!(keepers, 1);

        let positions = [
            Position::Goalkeeper,
            Position::Defender,
            Position::Defender,
            Position::Midfielder,
            Position::Midfielder,
            Position::Forward,
        ];
        let full: Vec<Player> = positions
            .iter()
            .enumerate()
            .map(|(index, position)| {
                let mut player = player(&format!("p{index}"), "user", "2010-01-01");
                player.position = position.clone();
                player
            })
            .collect();
        let plan = intake_plan_for_tier(&full, AcademyTier::CategoryOne);
        assert!(plan.groups.len() <= 2, "{:?}", plan.groups);
    }

    /// Given an AI scholar his club passes over (potential 40) whose deadline arrives
    /// When transitions run on that day and the next
    /// Then the decision is pushed a year out instead of rerunning daily.
    #[test]
    fn a_scholar_an_ai_club_passes_over_is_deferred_a_year() {
        let mut game = world();
        game.players[0].team_id = Some("rival".to_string());
        game.players[0].potential = 40;
        process_scholarship_transitions(&mut game);
        game.clock.current_date += chrono::Duration::days(30);

        process_scholarship_transitions(&mut game);

        assert_eq!(
            game.players[0].academy.scholarship_deadline.as_deref(),
            Some("2027-08-31")
        );
        let after_first = game.players[0].academy.scholarship_deadline.clone();
        game.clock.current_date += chrono::Duration::days(1);
        process_scholarship_transitions(&mut game);
        assert_eq!(game.players[0].academy.scholarship_deadline, after_first);
        assert_eq!(game.players[0].squad_role, SquadRole::Youth);
    }

    /// Given a first-year academy player poached the same season he arrived
    /// When the rival pays compensation
    /// Then at least one year is charged.
    #[test]
    fn a_first_year_academy_player_is_not_poached_for_nothing() {
        let mut game = world();
        game.players[0].date_of_birth = "2009-08-01".to_string();
        game.players[0].academy.joined_on = Some("2026-06-01".to_string());

        let fee = poach_academy_player(&mut game, "scholar", "rival").unwrap();

        assert!(fee > 0);
    }
}
