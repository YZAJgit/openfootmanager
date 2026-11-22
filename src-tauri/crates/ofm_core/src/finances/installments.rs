//! Transfer fees paid in instalments, and the straight-line amortization of a fee.
//!
//! A deal with tranches posts its upfront part when the player moves
//! (`transfers::execution`); each later tranche sits in `Game::transfer_installments` until it
//! falls due and the weekly settlement here posts it. A tranche is a balanced pair of journal
//! posts, the buyer's debit and the seller's credit, keyed on the tranche, so the money a deal
//! moves is conserved and a tranche can never be paid twice.
//!
//! A tranche is a contractual debt, as a wage is: it is paid when due whether or not the buyer
//! has the cash, and a buyer that cannot cover it goes into debt like one that cannot cover its
//! wage bill. It stays unpaid only when it cannot be posted at all (a club that no longer
//! exists, an amount past `i64`), and is retried every week until it can.

use std::collections::HashSet;

use chrono::NaiveDate;
use domain::finance::{CashKind, TransferInstallment};

use super::journal::{PostRequest, post_all};
use crate::game::Game;

const INSTALLMENT_POST_PREFIX: &str = "installment-";

/// Post every unsettled tranche due on or before `today`, oldest first, and mark each one settled
/// once its pair of posts has landed. Returns how many were settled.
pub fn settle_due_installments(game: &mut Game, today: NaiveDate) -> usize {
    let posted = posted_installment_ids(game);
    let mut settled = 0;
    for index in 0..game.transfer_installments.len() {
        let installment = &game.transfer_installments[index];
        if installment.settled || !is_due(installment, today) {
            continue;
        }
        let (debit_id, credit_id) = post_ids(&installment.id);
        if posted.contains(&debit_id) || posted.contains(&credit_id) {
            // The pair landed in an earlier run whose flag was lost: paying it again would
            // move the money twice.
            game.transfer_installments[index].settled = true;
            continue;
        }
        let installment_id = installment.id.clone();
        let reqs = installment_posts(installment, today);
        match post_all(game, &reqs) {
            Ok(_) => {
                game.transfer_installments[index].settled = true;
                settled += 1;
            }
            Err(err) => log::error!("instalment {installment_id} could not be posted: {err}"),
        }
    }
    settled
}

/// What `club_id` still owes in tranches not yet paid, in whole euros.
pub fn outstanding_installments_owed(game: &Game, club_id: &str) -> i64 {
    game.transfer_installments
        .iter()
        .filter(|installment| !installment.settled && installment.buyer_team_id == club_id)
        .fold(0i64, |total, installment| {
            total.saturating_add(installment.amount)
        })
}

/// The yearly charge of writing a fee off evenly over the contract it bought: `fee / years`,
/// the whole fee in one year for a contract shorter than a year.
pub fn straight_line_annual_amortization(fee: i64, contract_years: u32) -> i64 {
    fee / i64::from(contract_years.max(1))
}

/// What is left of a fee on the books on `as_of`, written off evenly by the day from
/// `contract_start` to `contract_end`. The whole fee before the contract starts, nothing once it
/// has ended (or when it ends on the day it starts).
pub fn amortized_book_value(
    fee: i64,
    contract_start: NaiveDate,
    contract_end: NaiveDate,
    as_of: NaiveDate,
) -> i64 {
    let total_days = (contract_end - contract_start).num_days();
    if total_days <= 0 || as_of >= contract_end {
        return 0;
    }
    let elapsed = (as_of - contract_start).num_days().clamp(0, total_days);
    let remaining = i128::from(fee) * i128::from(total_days - elapsed) / i128::from(total_days);
    // `remaining` lies between 0 and `fee`, so it fits.
    i64::try_from(remaining).unwrap_or(fee)
}

fn is_due(installment: &TransferInstallment, today: NaiveDate) -> bool {
    NaiveDate::parse_from_str(&installment.due_date, "%Y-%m-%d").is_ok_and(|due| due <= today)
}

fn installment_posts(installment: &TransferInstallment, today: NaiveDate) -> [PostRequest; 2] {
    let (debit_id, credit_id) = post_ids(&installment.id);
    [
        PostRequest::new(
            &installment.buyer_team_id,
            installment.amount.saturating_neg(),
            CashKind::TransferFeeOut,
            today,
        )
        .with_id(debit_id),
        PostRequest::new(
            &installment.seller_team_id,
            installment.amount,
            CashKind::TransferFeeIn,
            today,
        )
        .with_id(credit_id),
    ]
}

fn post_ids(installment_id: &str) -> (String, String) {
    (
        format!("{INSTALLMENT_POST_PREFIX}{installment_id}-out"),
        format!("{INSTALLMENT_POST_PREFIX}{installment_id}-in"),
    )
}

fn posted_installment_ids(game: &Game) -> HashSet<String> {
    game.cash_journal
        .iter()
        .filter(|post| post.id.starts_with(INSTALLMENT_POST_PREFIX))
        .map(|post| post.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::GameClock;
    use chrono::{TimeZone, Utc};
    use domain::manager::Manager;
    use domain::team::Team;

    fn club(id: &str, finance: i64) -> Team {
        let mut team = Team::new(
            id.to_string(),
            id.to_string(),
            id[..3].to_string(),
            "England".to_string(),
            "Town".to_string(),
            "Ground".to_string(),
            20_000,
        );
        team.finance = finance;
        team
    }

    fn game() -> Game {
        let clock = GameClock::new(Utc.with_ymd_and_hms(2026, 9, 7, 12, 0, 0).unwrap());
        let mut manager = Manager::new(
            "mgr".to_string(),
            "Alex".to_string(),
            "Boss".to_string(),
            "1980-01-01".to_string(),
            "England".to_string(),
        );
        manager.hire("buyer".to_string());
        Game::new(
            clock,
            manager,
            vec![club("buyer", 3_000_000), club("seller", 1_000_000)],
            vec![],
            vec![],
            vec![],
        )
    }

    fn tranche(id: &str, amount: i64, due_date: &str) -> TransferInstallment {
        TransferInstallment {
            id: id.to_string(),
            buyer_team_id: "buyer".to_string(),
            seller_team_id: "seller".to_string(),
            player_id: "p1".to_string(),
            amount,
            due_date: due_date.to_string(),
            settled: false,
            created_date: "2026-07-01".to_string(),
        }
    }

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn cash(game: &Game, club_id: &str) -> i64 {
        game.teams.iter().find(|t| t.id == club_id).unwrap().finance
    }

    /// Given one tranche due last week, one due today and one due next month
    /// When the weekly settlement runs
    /// Then the two due ones are paid, the buyer loses exactly what the seller gains, and the
    ///      one not yet due is left owed.
    #[test]
    fn due_tranches_move_the_same_money_out_of_the_buyer_and_into_the_seller() {
        let mut game = game();
        game.transfer_installments = vec![
            tranche("a", 1_000_000, "2026-08-31"),
            tranche("b", 1_000_000, "2026-09-07"),
            tranche("c", 1_000_000, "2026-10-07"),
        ];

        let settled = settle_due_installments(&mut game, day("2026-09-07"));

        assert_eq!(settled, 2);
        assert_eq!(cash(&game, "buyer"), 1_000_000);
        assert_eq!(cash(&game, "seller"), 3_000_000);
        let flags: Vec<bool> = game
            .transfer_installments
            .iter()
            .map(|i| i.settled)
            .collect();
        assert_eq!(flags, [true, true, false]);
        assert_eq!(outstanding_installments_owed(&game, "buyer"), 1_000_000);
        let net: i64 = game
            .cash_journal
            .iter()
            .filter(|post| post.id.starts_with(INSTALLMENT_POST_PREFIX))
            .map(|post| post.amount)
            .sum();
        assert_eq!(net, 0, "every tranche is a balanced pair");
        assert!(crate::finances::journal_matches_cash(&game));
    }

    /// Given a tranche that has been settled
    /// When the settlement runs again, and again after its flag is lost
    /// Then it is never paid a second time.
    #[test]
    fn a_tranche_is_settled_once() {
        let mut game = game();
        game.transfer_installments = vec![tranche("a", 500_000, "2026-09-01")];
        settle_due_installments(&mut game, day("2026-09-07"));
        let after_first = (cash(&game, "buyer"), cash(&game, "seller"));

        assert_eq!(settle_due_installments(&mut game, day("2026-09-14")), 0);
        game.transfer_installments[0].settled = false;
        assert_eq!(settle_due_installments(&mut game, day("2026-09-14")), 0);

        assert!(game.transfer_installments[0].settled);
        assert_eq!((cash(&game, "buyer"), cash(&game, "seller")), after_first);
    }

    /// Given a buyer with less cash than a due tranche
    /// When the settlement runs
    /// Then the tranche is still paid and the buyer goes into debt, as with an unpaid wage bill;
    ///      the journal still matches cash.
    #[test]
    fn an_unfunded_buyer_still_pays_and_goes_into_debt() {
        let mut game = game();
        game.transfer_installments = vec![tranche("a", 5_000_000, "2026-09-07")];

        assert_eq!(settle_due_installments(&mut game, day("2026-09-07")), 1);

        assert_eq!(cash(&game, "buyer"), -2_000_000);
        assert_eq!(cash(&game, "seller"), 6_000_000);
        assert!(crate::finances::journal_matches_cash(&game));
    }

    /// Given a due tranche whose seller no longer exists
    /// When the settlement runs
    /// Then nothing moves and the tranche stays owed, to be tried again next week.
    #[test]
    fn a_tranche_that_cannot_be_posted_stays_owed() {
        let mut game = game();
        let mut orphan = tranche("a", 500_000, "2026-09-07");
        orphan.seller_team_id = "gone".to_string();
        game.transfer_installments = vec![orphan];

        assert_eq!(settle_due_installments(&mut game, day("2026-09-07")), 0);

        assert!(!game.transfer_installments[0].settled);
        assert_eq!(cash(&game, "buyer"), 3_000_000);
        assert!(game.cash_journal.is_empty());
    }

    /// Given a 12 million fee over a four-year contract
    /// When it is amortized straight-line
    /// Then it costs 3 million a year, its book value halves at the half-way day and is gone
    ///      at the end.
    #[test]
    fn a_fee_is_written_off_evenly_over_the_contract() {
        assert_eq!(straight_line_annual_amortization(12_000_000, 4), 3_000_000);
        assert_eq!(straight_line_annual_amortization(12_000_000, 0), 12_000_000);
        let start = day("2026-07-01");
        // Four years of 365 days, so the half-way day falls on a whole day.
        let end = start + chrono::Duration::days(4 * 365);
        let middle = start + chrono::Duration::days(2 * 365);
        assert_eq!(
            amortized_book_value(12_000_000, start, end, start),
            12_000_000
        );
        assert_eq!(
            amortized_book_value(12_000_000, start, end, middle),
            6_000_000
        );
        assert_eq!(amortized_book_value(12_000_000, start, end, end), 0);
        assert_eq!(
            amortized_book_value(12_000_000, start, end, day("2026-01-01")),
            12_000_000
        );
        assert_eq!(amortized_book_value(12_000_000, start, start, start), 0);
    }
}
