//! Bids whose fee is paid partly up front and partly in later tranches.
//!
//! A structured bid is an ordinary outgoing bid (`bids.rs`) with two differences. The seller
//! judges it by what it is worth today, so a tranche due in a year counts for a little less than
//! the same money now; and the buyer's cash only has to cover the upfront part on the day of the
//! move, while the whole fee still comes out of its transfer budget. Once agreed, the upfront
//! part is posted with the move and each tranche waits in `Game::transfer_installments` for the
//! weekly settlement (`finances::settle_due_installments`).
//!
//! Only a move registered on the day can be structured: a deal held over for the window would
//! have to keep its tranches on the offer until it registers, which nothing stores.

use chrono::{Duration, NaiveDate};

use super::bids::{BidMoney, prepare_outgoing_bid, resolve_outgoing_bid};
use super::*;

/// A bid may split its fee into at most this many tranches after the upfront part.
const MAX_TRANCHES: u8 = 6;
/// Tranches are at least a month and at most a year apart.
const MIN_TRANCHE_INTERVAL_DAYS: u16 = 30;
const MAX_TRANCHE_INTERVAL_DAYS: u16 = 365;
/// How much less a seller values money paid a year late, in basis points: 8% a year, simple.
const SELLER_DISCOUNT_BASIS_POINTS_PER_YEAR: u128 = 800;

/// How a bid spreads its fee: `upfront_fee` on the day of the move, the rest in `tranche_count`
/// equal tranches, the first `interval_days` after the move and each later one `interval_days`
/// after the one before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallmentTerms {
    pub upfront_fee: u64,
    pub tranche_count: u8,
    pub interval_days: u16,
}

/// Why a spread of a fee cannot be offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallmentTermsError {
    /// No tranches, or more than [`MAX_TRANCHES`].
    TrancheCount,
    /// Tranches closer than a month or further apart than a year.
    TrancheInterval,
    /// The upfront part is the whole fee or more: there is nothing left to spread.
    NothingDeferred,
    /// The fee is beyond what the club accounts can hold.
    FeeTooLarge,
}

/// Why a structured bid was not made: its terms, or the refusal an outright bid would get (a
/// `be.error.*` key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuredBidError {
    Terms(InstallmentTermsError),
    Bid(String),
}

/// One later tranche of a fee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScheduledTranche {
    pub(super) amount: i64,
    pub(super) due_date: NaiveDate,
}

/// A fee split into its upfront part and its tranches. The parts always add up to the fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InstallmentSchedule {
    upfront: i64,
    tranches: Vec<ScheduledTranche>,
}

impl InstallmentSchedule {
    /// Split `total_fee` by `terms`, the tranches falling due from `move_date`. The deferred part
    /// is shared evenly, the last tranche taking what an even split leaves over.
    pub(super) fn build(
        total_fee: u64,
        terms: InstallmentTerms,
        move_date: NaiveDate,
    ) -> Result<Self, InstallmentTermsError> {
        if terms.tranche_count == 0 || terms.tranche_count > MAX_TRANCHES {
            return Err(InstallmentTermsError::TrancheCount);
        }
        if !(MIN_TRANCHE_INTERVAL_DAYS..=MAX_TRANCHE_INTERVAL_DAYS).contains(&terms.interval_days) {
            return Err(InstallmentTermsError::TrancheInterval);
        }
        let total = i64::try_from(total_fee).map_err(|_| InstallmentTermsError::FeeTooLarge)?;
        let upfront =
            i64::try_from(terms.upfront_fee).map_err(|_| InstallmentTermsError::NothingDeferred)?;
        if upfront >= total {
            return Err(InstallmentTermsError::NothingDeferred);
        }
        let deferred = total - upfront;
        let count = i64::from(terms.tranche_count);
        let even_share = deferred / count;
        let tranches = (1..=count)
            .map(|number| ScheduledTranche {
                amount: if number == count {
                    deferred - even_share * (count - 1)
                } else {
                    even_share
                },
                due_date: move_date + Duration::days(number * i64::from(terms.interval_days)),
            })
            .collect();
        Ok(Self { upfront, tranches })
    }

    /// What changes hands on the day of the move.
    pub(super) fn upfront(&self) -> i64 {
        self.upfront
    }

    pub(super) fn tranches(&self) -> &[ScheduledTranche] {
        &self.tranches
    }

    /// What the whole deal is worth to the seller on `today`: the upfront part in full, each
    /// tranche less 8% a year for the time it is waited for.
    pub(super) fn seller_value(&self, today: NaiveDate) -> u64 {
        let deferred_value = self
            .tranches
            .iter()
            .map(|tranche| discounted(tranche.amount, (tranche.due_date - today).num_days()))
            .fold(0u128, u128::saturating_add);
        let upfront = u128::try_from(self.upfront).unwrap_or(0);
        u64::try_from(upfront.saturating_add(deferred_value)).unwrap_or(u64::MAX)
    }
}

fn discounted(amount: i64, days_waited: i64) -> u128 {
    let amount = u128::try_from(amount).unwrap_or(0);
    let days = u128::try_from(days_waited).unwrap_or(0);
    let discount = (SELLER_DISCOUNT_BASIS_POINTS_PER_YEAR * days).min(365 * 10_000);
    amount * (365 * 10_000 - discount) / (365 * 10_000)
}

/// Bid `total_fee` for a player, paid as `terms` spread it. The seller answers as it would an
/// outright bid of what the spread is worth to it ([`InstallmentSchedule::seller_value`]); the
/// buyer's cash need only cover the upfront part. Refused while the window is closed.
pub fn make_structured_transfer_bid(
    game: &mut Game,
    player_id: &str,
    total_fee: u64,
    terms: InstallmentTerms,
) -> Result<TransferNegotiationOutcome, StructuredBidError> {
    expire_stale_transfer_offers(game);
    let today = game.clock.current_date.date_naive();
    let schedule =
        InstallmentSchedule::build(total_fee, terms, today).map_err(StructuredBidError::Terms)?;
    let money = BidMoney {
        valued: schedule.seller_value(today),
        cash: terms.upfront_fee,
        budget: total_fee,
    };
    let mut context =
        prepare_outgoing_bid(game, player_id, money).map_err(StructuredBidError::Bid)?;
    if !context.register_immediately {
        return Err(StructuredBidError::Bid(ERR_TRANSFER_WINDOW_CLOSED.into()));
    }
    context.schedule = Some(schedule);
    resolve_outgoing_bid(game, player_id, total_fee, context).map_err(StructuredBidError::Bid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn terms(upfront_fee: u64, tranche_count: u8, interval_days: u16) -> InstallmentTerms {
        InstallmentTerms {
            upfront_fee,
            tranche_count,
            interval_days,
        }
    }

    /// Given 10 million with 4 million up front in three tranches a year apart
    /// When it is scheduled
    /// Then the three tranches are 2 million each, a year apart, and all the parts add up to 10
    ///      million.
    #[test]
    fn a_fee_is_spread_into_equal_tranches_that_add_up_to_it() {
        let schedule =
            InstallmentSchedule::build(10_000_000, terms(4_000_000, 3, 365), day("2026-07-01"))
                .unwrap();

        assert_eq!(schedule.upfront(), 4_000_000);
        let amounts: Vec<i64> = schedule.tranches().iter().map(|t| t.amount).collect();
        assert_eq!(amounts, [2_000_000, 2_000_000, 2_000_000]);
        let dues: Vec<NaiveDate> = schedule.tranches().iter().map(|t| t.due_date).collect();
        assert_eq!(
            dues,
            [day("2027-07-01"), day("2028-06-30"), day("2029-06-30")]
        );
    }

    /// Given a deferred part that does not split evenly
    /// When it is scheduled
    /// Then the last tranche takes what is left over, so nothing is lost to rounding.
    #[test]
    fn the_last_tranche_takes_the_remainder() {
        let schedule =
            InstallmentSchedule::build(1_000_000, terms(0, 3, 30), day("2026-07-01")).unwrap();

        let amounts: Vec<i64> = schedule.tranches().iter().map(|t| t.amount).collect();
        assert_eq!(amounts, [333_333, 333_333, 333_334]);
        assert_eq!(amounts.iter().sum::<i64>(), 1_000_000);
    }

    /// Given terms with no tranches, too many, a week apart, two years apart, or an upfront
    /// part that is the whole fee
    /// When they are scheduled
    /// Then each is refused for its own reason.
    #[test]
    fn terms_that_cannot_be_spread_are_refused() {
        let move_date = day("2026-07-01");
        let build = |t| InstallmentSchedule::build(1_000_000, t, move_date);
        assert_eq!(
            build(terms(0, 0, 90)),
            Err(InstallmentTermsError::TrancheCount)
        );
        assert_eq!(
            build(terms(0, 7, 90)),
            Err(InstallmentTermsError::TrancheCount)
        );
        assert_eq!(
            build(terms(0, 2, 7)),
            Err(InstallmentTermsError::TrancheInterval)
        );
        assert_eq!(
            build(terms(0, 2, 730)),
            Err(InstallmentTermsError::TrancheInterval)
        );
        assert_eq!(
            build(terms(1_000_000, 2, 90)),
            Err(InstallmentTermsError::NothingDeferred)
        );
        assert_eq!(
            InstallmentSchedule::build(u64::MAX, terms(0, 2, 90), move_date),
            Err(InstallmentTermsError::FeeTooLarge)
        );
    }

    fn cash(game: &Game, club_id: &str) -> i64 {
        game.teams.iter().find(|t| t.id == club_id).unwrap().finance
    }

    /// Given a user club with 5 million who bids 1 million for a 600,000 player, 400,000 up front
    /// and two tranches of 300,000 six months apart
    /// When the seller answers
    /// Then the deal is agreed, the player moves, only the upfront part changes hands, both
    ///      tranches are owed by the buyer to the seller, and the whole fee leaves the budget.
    #[test]
    fn an_agreed_structured_bid_pays_up_front_and_owes_the_rest() {
        let mut game = super::super::tests::acceptance_game("team2", false);

        let outcome = make_structured_transfer_bid(
            &mut game,
            "player-award",
            1_000_000,
            terms(400_000, 2, 182),
        )
        .unwrap();

        assert_eq!(outcome.decision, TransferNegotiationDecision::Accepted);
        assert_eq!(game.players[0].team_id.as_deref(), Some("team1"));
        assert_eq!(cash(&game, "team1"), 4_600_000);
        assert_eq!(cash(&game, "team2"), 5_400_000);
        let owed: Vec<(i64, &str, &str)> = game
            .transfer_installments
            .iter()
            .map(|i| {
                (
                    i.amount,
                    i.buyer_team_id.as_str(),
                    i.seller_team_id.as_str(),
                )
            })
            .collect();
        assert_eq!(
            owed,
            [(300_000, "team1", "team2"), (300_000, "team1", "team2")]
        );
        assert_eq!(game.transfer_installments[0].due_date, "2026-07-13");
        assert!(game.transfer_installments.iter().all(|i| !i.settled));
        let budget = game
            .teams
            .iter()
            .find(|t| t.id == "team1")
            .unwrap()
            .transfer_budget;
        assert_eq!(budget, 4_000_000);
        assert!(crate::finances::journal_matches_cash(&game));
    }

    /// Given a buyer with 500,000 in cash and a big transfer budget
    /// When it bids 1.2 million outright, and then the same with 400,000 up front
    /// Then the outright bid is refused for want of cash, and the spread one is agreed.
    #[test]
    fn spreading_a_fee_lets_a_club_bid_beyond_its_cash() {
        let mut game = super::super::tests::acceptance_game("team2", false);
        game.teams[0].finance = 500_000;

        assert_eq!(
            make_transfer_bid(&mut game, "player-award", 1_200_000),
            Err(ERR_INSUFFICIENT_FUNDS.to_string())
        );
        let outcome = make_structured_transfer_bid(
            &mut game,
            "player-award",
            1_200_000,
            terms(400_000, 2, 182),
        )
        .unwrap();

        assert_eq!(outcome.decision, TransferNegotiationDecision::Accepted);
        assert_eq!(cash(&game, "team1"), 100_000);
    }

    /// Given a closed window
    /// When a structured bid is made
    /// Then it is refused as an outright one would be scheduled, and nothing is owed.
    #[test]
    fn a_structured_bid_needs_an_open_window() {
        let mut game = super::super::tests::acceptance_game("team2", true);

        let refusal = make_structured_transfer_bid(
            &mut game,
            "player-award",
            1_000_000,
            terms(400_000, 2, 182),
        );

        assert_eq!(
            refusal,
            Err(StructuredBidError::Bid(
                ERR_TRANSFER_WINDOW_CLOSED.to_string()
            ))
        );
        assert!(game.transfer_installments.is_empty());
        assert!(game.players[0].transfer_offers.is_empty());
    }

    /// Given 10 million, half up front and half a year later
    /// When the seller values it on the day of the move
    /// Then the tranche counts 8% less: 9.6 million in all.
    #[test]
    fn a_seller_values_a_later_tranche_at_less_than_its_face() {
        let move_date = day("2026-07-01");
        let schedule =
            InstallmentSchedule::build(10_000_000, terms(5_000_000, 1, 365), move_date).unwrap();

        assert_eq!(schedule.seller_value(move_date), 9_600_000);
    }
}
