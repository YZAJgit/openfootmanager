//! The daily sweep: who wants whom, and which of them gets to ask.
//!
//! This is the top of the call graph. It scores every player once, sorts the attractive ones,
//! then walks the buying clubs and lets each open at most one approach — subject to the
//! budget in `lifecycle`, which is what keeps a listed player from being buried in offers.

use super::*;

use crate::squad_floor::group_index as position_group_index;

/// Whether a club has a realistic reason to pursue a target: it isn't far below
/// the player's current club in stature, and it isn't already overloaded in the
/// player's position group.
pub(crate) fn buyer_has_genuine_interest(
    buyer_reputation: u32,
    owner_reputation: u32,
    buyer_position_depth: usize,
) -> bool {
    let reputation_deficit = owner_reputation as i32 - buyer_reputation as i32;
    reputation_deficit <= MAX_BUYER_REPUTATION_DEFICIT
        && buyer_position_depth < POSITION_GROUP_SURPLUS_THRESHOLD
}
/// Current squad depth per club and broad position group, computed once so the
/// market sweep doesn't re-scan every roster. The same count the squad floor
/// keeps, owned so the sweep can go on to move players.
pub(crate) fn squad_position_depths(game: &Game) -> std::collections::HashMap<String, [usize; 4]> {
    crate::squad_floor::registered_by_club(game)
        .into_iter()
        .map(|(team_id, depths)| (team_id.to_string(), depths))
        .collect()
}
pub(crate) fn contract_days_remaining(
    current_date: NaiveDate,
    contract_end: Option<&str>,
) -> Option<i64> {
    let contract_end = contract_end?;
    let contract_end_date = NaiveDate::parse_from_str(contract_end, "%Y-%m-%d").ok()?;
    Some((contract_end_date - current_date).num_days())
}
pub(crate) fn infer_player_importance(
    player: &domain::player::Player,
    owner_team: &domain::team::Team,
) -> PlayerImportance {
    if owner_team.starting_xi_ids.iter().any(|id| id == &player.id) {
        return PlayerImportance::Key;
    }

    if player.market_value >= 1_500_000 {
        return PlayerImportance::Regular;
    }

    PlayerImportance::Fringe
}
pub(crate) fn minimum_acceptable_fee(
    current_date: NaiveDate,
    player: &domain::player::Player,
    owner_team: &domain::team::Team,
    buyer_team: &domain::team::Team,
) -> u64 {
    let mut multiplier: f64 = if player.transfer_listed { 0.8 } else { 1.2 };

    if let Some(days_remaining) = contract_days_remaining(current_date, player.contract_end()) {
        if days_remaining <= 60 {
            multiplier -= 0.25;
        } else if days_remaining <= 180 {
            multiplier -= 0.15;
        } else if days_remaining <= 365 {
            multiplier -= 0.05;
        }
    }

    match infer_player_importance(player, owner_team) {
        PlayerImportance::Key => multiplier += 0.2,
        PlayerImportance::Regular => multiplier += 0.1,
        PlayerImportance::Fringe => {}
    }

    if player.morale <= 40 {
        multiplier -= 0.05;
    }

    let openness_score = player_move_openness_score(current_date, player, owner_team, buyer_team);
    if openness_score >= 60 {
        multiplier -= 0.20;
    } else if openness_score >= 40 {
        multiplier -= 0.10;
    }

    let multiplier = multiplier.clamp(0.55, 1.6);
    let asking = ((player.market_value as f64) * multiplier).round() as u64;
    // A club never asks more than the clause it signed: any bid at the clause buys him.
    release_clause_fee(player).map_or(asking, |clause| asking.min(clause))
}
/// The fee that triggers `player`'s release clause, when his contract has one. A clause of zero or
/// less is no clause.
pub(crate) fn release_clause_fee(player: &domain::player::Player) -> Option<u64> {
    player
        .release_clause
        .and_then(|clause| u64::try_from(clause).ok())
        .filter(|clause| *clause > 0)
}
/// Whether `fee` meets `player`'s release clause. Such a bid binds his club: it can neither haggle
/// over the fee nor turn it down.
pub(crate) fn release_clause_met(player: &domain::player::Player, fee: u64) -> bool {
    release_clause_fee(player).is_some_and(|clause| fee >= clause)
}
pub(crate) fn player_move_openness_score(
    current_date: NaiveDate,
    player: &domain::player::Player,
    owner_team: &domain::team::Team,
    buyer_team: &domain::team::Team,
) -> i32 {
    let mut score = 0;

    if player.morale <= 45 {
        score += 20;
    } else if player.morale <= 60 {
        score += 10;
    }

    if player.stats.appearances <= 2 {
        score += 15;
    } else if player.stats.appearances <= 5 {
        score += 8;
    }

    if let Some(days_remaining) = contract_days_remaining(current_date, player.contract_end()) {
        if days_remaining <= 180 {
            score += 20;
        } else if days_remaining <= 365 {
            score += 10;
        }
    }

    let reputation_gap = buyer_team.reputation as i32 - owner_team.reputation as i32;
    if reputation_gap >= 200 {
        score += 25;
    } else if reputation_gap >= 75 {
        score += 15;
    }

    if player.transfer_listed {
        score += 10;
    }

    score
}
pub(crate) fn apply_blocked_move_consequences(
    player: &mut domain::player::Player,
    openness_score: i32,
) {
    if openness_score < 40 {
        return;
    }

    let morale_drop = if openness_score >= 60 { 10 } else { 6 };
    player.morale = (i16::from(player.morale) - morale_drop).clamp(0, 100) as u8;
    player.morale_core.manager_trust =
        (i16::from(player.morale_core.manager_trust) - 5).clamp(0, 100) as u8;
    player.morale_core.unresolved_issue = Some(domain::player::PlayerIssue {
        category: domain::player::PlayerIssueCategory::Contract,
        severity: if openness_score >= 60 { 75 } else { 60 },
    });
}
pub(crate) fn award_leaderboard_player_ids(game: &Game) -> HashSet<String> {
    let awards = crate::season_awards::compute_season_awards(game);

    awards
        .golden_boot
        .iter()
        .chain(awards.assist_king.iter())
        .chain(awards.player_of_year.iter())
        .chain(awards.clean_sheet_king.iter())
        .chain(awards.most_appearances.iter())
        .chain(awards.young_player.iter())
        .map(|entry| entry.player_id.clone())
        .collect()
}
pub fn evaluate_transfer_market(game: &mut Game) {
    expire_stale_transfer_offers(game);
    expire_stale_loan_offers(game);
    prune_closed_offers(game);
    if !transfer_window_is_open(game) {
        return;
    }
    let mut sweep = MarketSweep::new(game);
    for buyer_id in market_buyer_ids(game, sweep.user_team_id.as_deref()) {
        sweep.visit_buyer(game, &buyer_id);
    }
    escalate_rival_bids(game);
}
/// A window this close to shutting has clubs hurrying to finish their business.
const DEADLINE_URGENCY_DAYS: i64 = 3;
/// How many of the best targets a club weighs against its squad's needs.
const SQUAD_PLANNING_SCAN_LIMIT: usize = 5;
/// The squad a club plans around, per position group (GK, DEF, MID, FWD).
const PLANNED_GROUP_DEPTH: [usize; 4] = [2, 6, 6, 4];
/// What each player a group is short of its plan adds to a target's appeal, up to three.
const THIN_GROUP_BONUS_PER_MISSING_PLAYER: i32 = 6;
const MAX_MISSING_PLAYERS_WEIGHED: usize = 3;

/// How clubs behave on a given day of the window: on deadline day, or in the last few days of
/// it, they open talks faster, close deals on less conviction and pay over the odds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MarketUrgency {
    /// How many new clubs may open talks for one user player in a day.
    pub(crate) new_offers_per_player_per_day: usize,
    /// An AI club completes a purchase only for a target more appealing than this.
    pub(crate) ai_deal_score_threshold: i32,
    /// What a club offers, as a percentage of its valuation, before it is capped at what it can pay.
    pub(crate) fee_premium_pct: u64,
}

impl MarketUrgency {
    pub(crate) fn for_window(window: &domain::season::TransferWindowContext) -> Self {
        let urgent = match window.status {
            TransferWindowStatus::DeadlineDay => true,
            TransferWindowStatus::Open => window
                .days_remaining
                .is_some_and(|days| days <= DEADLINE_URGENCY_DAYS),
            TransferWindowStatus::Closed => false,
        };
        if urgent {
            Self {
                new_offers_per_player_per_day: MAX_NEW_INCOMING_OFFERS_PER_USER_PLAYER_PER_DAY + 1,
                ai_deal_score_threshold: 50,
                fee_premium_pct: 110,
            }
        } else {
            Self {
                new_offers_per_player_per_day: MAX_NEW_INCOMING_OFFERS_PER_USER_PLAYER_PER_DAY,
                ai_deal_score_threshold: 60,
                fee_premium_pct: 100,
            }
        }
    }

    /// What a club offers for a player it values at `valuation`, never more than `ceiling`, the
    /// most it can pay (and never less than the valuation it could already afford).
    pub(crate) fn offered_fee(self, valuation: u64, ceiling: u64) -> u64 {
        let pushed = valuation.saturating_mul(self.fee_premium_pct) / 100;
        pushed.min(ceiling).max(valuation)
    }
}

/// How much a club's plans add to a target's appeal: more for a position group it is short in,
/// more for a young player it can build around, less for one in his thirties.
pub(crate) fn squad_planning_bonus(position_group: usize, group_depth: usize, age: i32) -> i32 {
    let planned = PLANNED_GROUP_DEPTH
        .get(position_group)
        .copied()
        .unwrap_or(0);
    let missing = planned
        .saturating_sub(group_depth)
        .min(MAX_MISSING_PLAYERS_WEIGHED);
    let need = i32::try_from(missing).unwrap_or(0) * THIN_GROUP_BONUS_PER_MISSING_PLAYER;
    let age_weight = match age {
        ..=21 => 8,
        22..=24 => 4,
        25..=29 => 0,
        _ => -6,
    };
    need + age_weight
}

/// The buyer the sweep is choosing a target for, and what it already knows about it.
struct BuyerView<'a> {
    team: &'a Team,
    depths: [usize; 4],
    committed_fees: i64,
    loan_offer_player_id: Option<&'a str>,
}
fn market_shortlist(
    game: &Game,
    user_team_id: Option<&str>,
    current_date: NaiveDate,
    position_depths: &std::collections::HashMap<String, [usize; 4]>,
) -> Vec<MarketTarget> {
    let award_leaderboards = award_leaderboard_player_ids(game);
    let team_reputation: std::collections::HashMap<String, u32> = game
        .teams
        .iter()
        .map(|team| (team.id.clone(), team.reputation))
        .collect();
    let mut shortlist: Vec<MarketTarget> = Vec::new();
    for player in &game.players {
        let Some(owner_team_id) = player.team_id.as_deref() else {
            continue;
        };
        if player_has_pending_registration(player) {
            continue;
        }
        // An AI club at the floor in his group will not sell him, so no buyer
        // should spend its approach on him. `execute_transfer` refuses the sale
        // regardless; this keeps the refusal from costing a buyer its day. The
        // player's own club is left to decide for itself.
        let is_user_owned = Some(owner_team_id) == user_team_id;
        if !is_user_owned
            && player.squad_role == domain::player::SquadRole::Senior
            && position_depths
                .get(owner_team_id)
                .is_some_and(|depths| !crate::squad_floor::can_spare_one(*depths, &player.position))
        {
            continue;
        }
        let mut score = incoming_interest_score(current_date, player);
        if award_leaderboards.contains(&player.id) {
            score += AWARD_LEADERBOARD_INTEREST_BONUS;
        }
        if score < 35 {
            continue;
        }
        shortlist.push(MarketTarget {
            player_id: player.id.clone(),
            owner_team_id: owner_team_id.to_string(),
            is_user_owned,
            score,
            fee: suggested_incoming_fee(current_date, player),
            position_group_index: position_group_index(&player.position),
            owner_reputation: team_reputation.get(owner_team_id).copied().unwrap_or(0),
            age: domain::age::age_on_or_unknown(current_date, &player.date_of_birth),
        });
    }
    // Highest appeal first; a stable sort preserves the original ordering among
    // equally appealing targets, so selection is unchanged.
    shortlist.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));

    shortlist
}
fn market_buyer_ids(game: &Game, user_team_id: Option<&str>) -> Vec<String> {
    let active_team_ids = game.active_team_ids();
    game.teams
        .iter()
        .filter(|team| Some(team.id.as_str()) != user_team_id)
        .filter(|team| {
            active_team_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&team.id))
        })
        .map(|team| team.id.clone())
        .collect()
}
struct MarketSweep {
    user_team_id: Option<String>,
    current_date: NaiveDate,
    today: String,
    position_depths: std::collections::HashMap<String, [usize; 4]>,
    wage_bills: std::collections::HashMap<String, i64>,
    player_index: std::collections::HashMap<String, usize>,
    completed_ai_transfers: usize,
    moved_player_ids: HashSet<String>,
    new_offers_per_player: std::collections::HashMap<String, usize>,
    new_loan_offers_per_player: std::collections::HashMap<String, usize>,
    new_user_offers_today: usize,
    new_user_loan_offers_today: usize,
    approach_clubs: std::collections::HashMap<String, HashSet<String>>,
    cooled_clubs: std::collections::HashMap<String, HashSet<String>>,
    shortlist: Vec<MarketTarget>,
    reservations_by_club: std::collections::HashMap<String, i64>,
    urgency: MarketUrgency,
}
impl MarketSweep {
    fn new(game: &Game) -> Self {
        let user_team_id = game.manager.team_id.clone();

        let current_date = game.clock.current_date.date_naive();
        let today = game.clock.current_date.format("%Y-%m-%d").to_string();
        let position_depths = squad_position_depths(game);
        // Every club's wage bill, read once: a buyer's check needs its own, and finding one scans
        // every player. The sweep moves players, so the two clubs of a completed transfer are
        // recounted below.
        let wage_bills = crate::finances::wage_bills_by_club(game);
        // Where each target sits in `game.players`, so the buyer's wage check need not search.
        // The sweep moves players between clubs but never adds or removes one.
        let player_index: std::collections::HashMap<String, usize> = game
            .players
            .iter()
            .enumerate()
            .map(|(index, player)| (player.id.clone(), index))
            .collect();

        let completed_ai_transfers = 0_usize;
        let moved_player_ids: HashSet<String> = HashSet::new();
        // New incoming offers opened to user players today, tracked to throttle the
        // inbox: at most one new club per player and a hard squad-wide ceiling.
        let new_offers_per_player: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let new_loan_offers_per_player: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let new_user_offers_today = 0_usize;
        let new_user_loan_offers_today = 0_usize;
        // Clubs already in talks for each user player, kept live rather than snapshotted so offers
        // opened earlier in today's sweep count against the ceiling too. One club is one approach,
        // so this both caps the queue and stops a club holding two approaches on the same player.
        let approach_clubs: std::collections::HashMap<String, HashSet<String>> = game
            .players
            .iter()
            .filter(|player| player.team_id.as_deref() == user_team_id.as_deref())
            .map(|player| (player.id.clone(), pending_approach_clubs(player)))
            .collect();
        // Clubs turned away recently. Unlike the queue above this is fixed for the day: a rejection
        // made today already carries its closure date, so it is in here from the next sweep onwards.
        let cooled_clubs: std::collections::HashMap<String, HashSet<String>> = game
            .players
            .iter()
            .filter(|player| player.team_id.as_deref() == user_team_id.as_deref())
            .map(|player| {
                (
                    player.id.clone(),
                    clubs_in_rebid_cooldown(player, current_date),
                )
            })
            .collect();

        let shortlist = market_shortlist(
            game,
            user_team_id.as_deref(),
            current_date,
            &position_depths,
        );
        let reservations_by_club = committed_transfer_fees_by_club(game);
        Self {
            user_team_id,
            current_date,
            today,
            position_depths,
            wage_bills,
            player_index,
            completed_ai_transfers,
            moved_player_ids,
            new_offers_per_player,
            new_loan_offers_per_player,
            new_user_offers_today,
            new_user_loan_offers_today,
            approach_clubs,
            cooled_clubs,
            shortlist,
            reservations_by_club,
            urgency: MarketUrgency::for_window(&game.season_context.transfer_window),
        }
    }
    fn offer_loan_to_user(&mut self, game: &mut Game, buyer_team: &Team) -> Option<String> {
        let buyer_id = buyer_team.id.as_str();
        let loan_offer_player_id = if let Some(user_team_id) = self.user_team_id.as_deref() {
            if self.new_user_loan_offers_today < MAX_NEW_INCOMING_USER_OFFERS_PER_DAY {
                create_incoming_user_loan_offer_if_any(
                    game,
                    user_team_id,
                    buyer_id,
                    &buyer_team.name,
                    &self.today,
                    self.current_date,
                    IncomingOfferBudget {
                        new_today: &self.new_loan_offers_per_player,
                        approach_clubs: &self.approach_clubs,
                        cooled_clubs: &self.cooled_clubs,
                    },
                )
            } else {
                None
            }
        } else {
            None
        };
        if let Some(player_id) = loan_offer_player_id.as_ref() {
            *self
                .new_loan_offers_per_player
                .entry(player_id.clone())
                .or_insert(0) += 1;
            self.approach_clubs
                .entry(player_id.clone())
                .or_default()
                .insert(buyer_id.to_string());
            self.new_user_loan_offers_today += 1;
        }

        loan_offer_player_id
    }
    fn candidate_for(
        &self,
        game: &Game,
        buyer_team: &Team,
        loan_offer_player_id: Option<&str>,
    ) -> Option<MarketCandidate> {
        let buyer_id = buyer_team.id.as_str();
        let view = BuyerView {
            team: buyer_team,
            depths: self
                .position_depths
                .get(buyer_id)
                .copied()
                .unwrap_or([0; 4]),
            committed_fees: self
                .reservations_by_club
                .get(buyer_id)
                .copied()
                .unwrap_or(0),
            loan_offer_player_id,
        };
        // Worked out once for this buyer, and only if a target gets as far as needing it.
        let mut buyer_wage_facts: Option<BuyerWageFacts> = None;
        // The list is score-sorted. The club weighs the best few it could sign against what its
        // squad needs, so a thin position or a young prospect can edge a slightly more appealing
        // player; the scan is bounded so a weak target far down the list never wins on need alone.
        let target = self
            .shortlist
            .iter()
            .filter(|target| self.target_is_eligible(game, &view, target, &mut buyer_wage_facts))
            .take(SQUAD_PLANNING_SCAN_LIMIT)
            .enumerate()
            .max_by_key(|(rank, target)| {
                let group_depth = view
                    .depths
                    .get(target.position_group_index)
                    .copied()
                    .unwrap_or(0);
                let planned = target.score
                    + squad_planning_bonus(target.position_group_index, group_depth, target.age);
                (planned, std::cmp::Reverse(*rank))
            })
            .map(|(_, target)| target)?;
        let ceiling = transfer_fee_ceiling(buyer_team, view.committed_fees);
        Some(MarketCandidate {
            player_id: target.player_id.clone(),
            owner_team_id: target.owner_team_id.clone(),
            score: target.score,
            fee: self.urgency.offered_fee(target.fee, ceiling),
        })
    }
    /// Whether the buyer in `view` could and would pursue `target` today.
    fn target_is_eligible(
        &self,
        game: &Game,
        view: &BuyerView<'_>,
        target: &MarketTarget,
        buyer_wage_facts: &mut Option<BuyerWageFacts>,
    ) -> bool {
        let buyer_id = view.team.id.as_str();
        if target.owner_team_id == buyer_id || self.moved_player_ids.contains(&target.player_id) {
            return false;
        }
        if view.loan_offer_player_id == Some(target.player_id.as_str()) {
            return false;
        }
        if target.is_user_owned {
            let budget = IncomingOfferBudget {
                new_today: &self.new_offers_per_player,
                approach_clubs: &self.approach_clubs,
                cooled_clubs: &self.cooled_clubs,
            };
            if !budget.accepts(
                &target.player_id,
                buyer_id,
                self.urgency.new_offers_per_player_per_day,
            ) || self.new_user_offers_today >= MAX_NEW_INCOMING_USER_OFFERS_PER_DAY
            {
                return false;
            }
        } else if self.completed_ai_transfers >= MAX_COMPLETED_AI_TRANSFERS_PER_DAY {
            return false;
        }
        // Clubs only chase players that fit their stature and a position they
        // actually need, so a single star doesn't draw the whole division.
        if !buyer_has_genuine_interest(
            view.team.reputation,
            target.owner_reputation,
            view.depths[target.position_group_index],
        ) {
            return false;
        }
        can_afford_fee(view.team, target.fee, view.committed_fees)
            && self
                .player_index
                .get(&target.player_id)
                .is_some_and(|&index| {
                    let facts = buyer_wage_facts.get_or_insert_with(|| {
                        BuyerWageFacts::new(
                            self.wage_bills.get(buyer_id).copied().unwrap_or(0),
                            view.depths,
                        )
                    });
                    buyer_can_pay_standard_wage(
                        &game.players[index],
                        view.team,
                        facts,
                        self.current_date,
                    )
                })
    }
    fn pursue_candidate(&mut self, game: &mut Game, buyer_team: &Team, candidate: MarketCandidate) {
        let buyer_id = buyer_team.id.as_str();

        if Some(candidate.owner_team_id.as_str()) == self.user_team_id.as_deref() {
            create_incoming_user_offer(game, &candidate, buyer_id, &buyer_team.name, &self.today);
            *self
                .new_offers_per_player
                .entry(candidate.player_id.clone())
                .or_insert(0) += 1;
            self.approach_clubs
                .entry(candidate.player_id.clone())
                .or_default()
                .insert(buyer_id.to_string());
            self.new_user_offers_today += 1;
            return;
        }

        if candidate.score <= self.urgency.ai_deal_score_threshold
            || self.completed_ai_transfers >= MAX_COMPLETED_AI_TRANSFERS_PER_DAY
        {
            return;
        }

        if execute_transfer(
            game,
            &candidate.player_id,
            buyer_id,
            &candidate.owner_team_id,
            candidate.fee,
        )
        .and_then(|()| {
            close_open_offers_after_sale(game, &candidate.player_id, buyer_id, candidate.fee, None)
        })
        .is_ok()
        {
            self.moved_player_ids.insert(candidate.player_id);
            self.completed_ai_transfers += 1;
            // Only the seller is recounted: a club that has just bought has had its turn (each
            // club is a buyer once a sweep), so its new bill is never read again today, while
            // the seller may still buy and is held to the bill it has after the sale.
            self.wage_bills.insert(
                candidate.owner_team_id.clone(),
                calc_wages(game, &candidate.owner_team_id),
            );
        }
    }
    fn visit_buyer(&mut self, game: &mut Game, buyer_id: &str) {
        let Some(buyer_team) = game.teams.iter().find(|team| team.id == buyer_id).cloned() else {
            return;
        };
        let loan_offer_player_id = self.offer_loan_to_user(game, &buyer_team);
        if let Some(candidate) =
            self.candidate_for(game, &buyer_team, loan_offer_player_id.as_deref())
        {
            self.pursue_candidate(game, &buyer_team, candidate);
        }
    }
}
pub fn generate_incoming_transfer_offers(game: &mut Game) {
    evaluate_transfer_market(game);
}
