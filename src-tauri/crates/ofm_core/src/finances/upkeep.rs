//! What a club pays to keep its facilities running every week, and to open its ground for a
//! home match.
//!
//! Upkeep is calibrated against what the facilities cost to build: a club at the starting level
//! of every facility pays nothing beyond its staff, and each upgrade carries a running cost of
//! about a fifth of its build cost a year. Staging a match costs a small, fixed sum per seat,
//! well under a tenth of what the gate takes for it, so an ordinary home match always nets money.

use domain::team::{Facilities, Team};

use crate::club::BASE_FACILITY_UPGRADE_COST;

/// Weekly running cost of a facility, in basis points of what was spent raising it above the
/// starting level. 40 a week is about 21% of the investment a year.
const FACILITY_WEEKLY_UPKEEP_BASIS_POINTS: i64 = 40;
/// Euros a home match costs per seat of the ground: stewarding, cleaning, the pitch, utilities.
/// A gate takes 9 to 23 euros a seat (`gate::expected_gate`), so this is under a ninth of it.
const STADIUM_UPKEEP_PER_SEAT_PER_MATCH: i64 = 1;

/// The club's weekly facility upkeep, in whole euros. Zero at the starting level of every
/// facility; the Monday wage run posts it as `CashKind::Upkeep`.
pub fn calc_upkeep(team: &Team) -> i64 {
    facility_upkeep(&team.facilities)
}

/// What staging one home match costs a ground of `capacity` seats, in whole euros.
pub fn stadium_matchday_upkeep(capacity: u32) -> i64 {
    i64::from(capacity).saturating_mul(STADIUM_UPKEEP_PER_SEAT_PER_MATCH)
}

fn facility_upkeep(facilities: &Facilities) -> i64 {
    let invested = [facilities.training, facilities.medical, facilities.scouting]
        .into_iter()
        .map(invested_above_starting_level)
        .fold(0i64, i64::saturating_add);
    invested.saturating_mul(FACILITY_WEEKLY_UPKEEP_BASIS_POINTS) / 10_000
}

/// What raising a facility from level 1 to `level` cost: upgrading from level L costs
/// `L * BASE_FACILITY_UPGRADE_COST` (`club::next_upgrade_cost`), so the sum is a triangle.
fn invested_above_starting_level(level: u8) -> i64 {
    let upgrades = i64::from(level.saturating_sub(1));
    BASE_FACILITY_UPGRADE_COST.saturating_mul(upgrades * (upgrades + 1) / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn club_with(training: u8, medical: u8, scouting: u8) -> Team {
        let mut team = Team::new(
            "club".to_string(),
            "Club".to_string(),
            "CLB".to_string(),
            "England".to_string(),
            "Town".to_string(),
            "Ground".to_string(),
            40_000,
        );
        team.facilities = Facilities {
            training,
            medical,
            scouting,
        };
        team
    }

    /// Given a club at the starting level of every facility
    /// When its weekly upkeep is worked out
    /// Then it is nothing: the starting facilities are run by the staff the club already pays.
    #[test]
    fn starting_facilities_cost_no_upkeep() {
        assert_eq!(calc_upkeep(&club_with(1, 1, 1)), 0);
    }

    /// Given a club whose training ground is one level up
    /// When its weekly upkeep is worked out
    /// Then it pays 40 basis points of the 250,000 the upgrade cost: 1,000 a week.
    #[test]
    fn one_upgrade_costs_forty_basis_points_of_its_price_a_week() {
        assert_eq!(calc_upkeep(&club_with(2, 1, 1)), 1_000);
    }

    /// Given a club with every facility at level 5
    /// When its weekly upkeep is worked out
    /// Then each facility costs 0.4% of the 2.5 million spent on it, 30,000 a week in all, and
    ///      a year of it is about a fifth of the investment.
    #[test]
    fn upkeep_grows_with_what_the_facilities_cost_to_build() {
        let weekly = calc_upkeep(&club_with(5, 5, 5));
        assert_eq!(weekly, 30_000);
        let invested = 3 * 2_500_000;
        let yearly_pct = weekly * 52 * 100 / invested;
        assert!((18..=23).contains(&yearly_pct), "{yearly_pct}% a year");
    }

    /// Given a 40,000-seat ground
    /// When a home match is staged
    /// Then it costs 40,000: one euro a seat, well under the cheapest gate it takes.
    #[test]
    fn a_home_match_costs_a_euro_a_seat() {
        assert_eq!(stadium_matchday_upkeep(40_000), 40_000);
        let cheapest_gate = crate::finances::gate::gate_takings(40_000, 60, 15).unwrap();
        assert!(stadium_matchday_upkeep(40_000) * 9 <= cheapest_gate);
    }
}
