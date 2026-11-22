//! Rival clubs bidding for the same player.
//!
//! Several clubs may hold a live bid for one of the user's players at once. Each day of an open
//! window, every rival that has been outbid looks at the best bid on the table and either goes
//! above it or walks away: it raises by about a twentieth, never past what it can pay
//! (`transfer_fee_ceiling`) or what it thinks the player is worth (a premium over its own
//! valuation), and withdraws when the best bid is already past that.

use super::*;

/// How far above the best bid on the table an outbid rival goes: 5%.
const RIVAL_RAISE_PCT: u64 = 105;
/// The most a rival will pay over what it values the player at: 30%.
const RIVAL_VALUATION_PREMIUM_PCT: u64 = 130;

/// The live bids for `player`, best first. Bids of the same fee keep the order they arrived in.
pub fn competing_bids(player: &Player) -> Vec<&domain::player::TransferOffer> {
    let mut bids: Vec<&domain::player::TransferOffer> = player
        .transfer_offers
        .iter()
        .filter(|offer| offer.status == TransferOfferStatus::Pending)
        .collect();
    bids.sort_by_key(|offer| std::cmp::Reverse(offer.fee));
    bids
}

/// What an outbid rival does about the best bid on the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RivalMove {
    Raise(u64),
    Withdraw,
}

/// The most `buyer` would pay for `player`: what it can afford, and no more than 30% over its
/// own valuation of him.
fn rival_ceiling(game: &Game, buyer: &Team, player: &Player) -> u64 {
    let today = game.clock.current_date.date_naive();
    let valuation = suggested_incoming_fee(today, player);
    let worth_ceiling = valuation.saturating_mul(RIVAL_VALUATION_PREMIUM_PCT) / 100;
    transfer_fee_ceiling(buyer, committed_transfer_fees(game, &buyer.id)).min(worth_ceiling)
}

/// What a rival bidding `own_fee` does when the best bid is `best_fee`; `None` when it leads.
fn rival_move(own_fee: u64, best_fee: u64, ceiling: u64) -> Option<RivalMove> {
    if own_fee >= best_fee {
        return None;
    }
    let raise =
        round_transfer_fee(best_fee.saturating_mul(RIVAL_RAISE_PCT) / 100).max(best_fee + 1);
    Some(if raise <= ceiling {
        RivalMove::Raise(raise)
    } else {
        RivalMove::Withdraw
    })
}

/// Let every outbid rival for each of the user's players raise or withdraw, once. Returns how many
/// bids changed.
pub(crate) fn escalate_rival_bids(game: &mut Game) -> usize {
    let Some(user_team_id) = game.manager.team_id.clone() else {
        return 0;
    };
    let today = game.clock.current_date.format("%Y-%m-%d").to_string();
    let moves = planned_rival_moves(game, &user_team_id);
    for (player_index, offer_id, rival_move) in &moves {
        let Some(offer) = game.players[*player_index]
            .transfer_offers
            .iter_mut()
            .find(|offer| &offer.id == offer_id)
        else {
            continue;
        };
        match rival_move {
            RivalMove::Raise(fee) => {
                offer.fee = *fee;
                // A raised bid is a new proposal: any counter the user made to the old one is
                // answered by it.
                offer.suggested_counter_fee = None;
            }
            RivalMove::Withdraw => {
                close_transfer_offer(offer, TransferOfferStatus::Withdrawn, &today);
            }
        }
    }
    moves.len()
}

/// The moves of every outbid rival, worked out against the bids as they stand before any is made.
fn planned_rival_moves(game: &Game, user_team_id: &str) -> Vec<(usize, String, RivalMove)> {
    let mut moves = Vec::new();
    for (player_index, player) in game.players.iter().enumerate() {
        if player.team_id.as_deref() != Some(user_team_id) {
            continue;
        }
        let bids = competing_bids(player);
        let Some(best_fee) = bids.first().map(|offer| offer.fee) else {
            continue;
        };
        for offer in bids.iter().skip(1) {
            let Some(buyer) = game.teams.iter().find(|team| team.id == offer.from_team_id) else {
                continue;
            };
            let ceiling = rival_ceiling(game, buyer, player);
            if let Some(rival_move) = rival_move(offer.fee, best_fee, ceiling) {
                moves.push((player_index, offer.id.clone(), rival_move));
            }
        }
    }
    moves
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bid(game: &mut Game, club: &str, fee: u64) -> String {
        super::super::bids::upsert_transfer_offer(
            &mut game.players[0],
            club,
            fee,
            TransferOfferStatus::Pending,
            "2026-01-12",
            None,
            1,
            None,
            None,
        )
    }

    fn rival_game() -> Game {
        let mut game = super::super::tests::make_game();
        let mut rival = game.teams[1].clone();
        rival.id = "team3".to_string();
        rival.name = "Gamma FC".to_string();
        game.teams.push(rival);
        game
    }

    fn fee_of(game: &Game, club: &str) -> (u64, TransferOfferStatus) {
        let offer = game.players[0]
            .transfer_offers
            .iter()
            .find(|offer| offer.from_team_id == club)
            .unwrap();
        (offer.fee, offer.status.clone())
    }

    /// Given two clubs bidding 500,000 and 600,000 for a user player
    /// When the bids are ranked
    /// Then the 600,000 bid comes first.
    #[test]
    fn competing_bids_are_ranked_best_first() {
        let mut game = rival_game();
        bid(&mut game, "team2", 500_000);
        bid(&mut game, "team3", 600_000);

        let fees: Vec<u64> = competing_bids(&game.players[0])
            .iter()
            .map(|o| o.fee)
            .collect();

        assert_eq!(fees, [600_000, 500_000]);
    }

    /// Given a rival outbid by 600,000 that can afford more and values the player at 600,000
    /// When rivals escalate
    /// Then it raises 5%, rounded up to the next 50,000 as every quoted fee is, to 650,000, and
    ///      the leader is left as it was.
    #[test]
    fn an_outbid_rival_raises_above_the_best_bid() {
        let mut game = rival_game();
        bid(&mut game, "team2", 500_000);
        bid(&mut game, "team3", 600_000);

        assert_eq!(escalate_rival_bids(&mut game), 1);

        assert_eq!(
            fee_of(&game, "team2"),
            (650_000, TransferOfferStatus::Pending)
        );
        assert_eq!(
            fee_of(&game, "team3"),
            (600_000, TransferOfferStatus::Pending)
        );
    }

    /// Given a rival whose cash would not cover a raise above the best bid
    /// When rivals escalate
    /// Then it withdraws, and is not left holding a bid it cannot pay.
    #[test]
    fn a_rival_that_cannot_go_higher_withdraws() {
        let mut game = rival_game();
        game.teams[1].finance = 610_000;
        bid(&mut game, "team2", 500_000);
        bid(&mut game, "team3", 600_000);

        escalate_rival_bids(&mut game);

        assert_eq!(fee_of(&game, "team2").1, TransferOfferStatus::Withdrawn);
        assert_eq!(
            fee_of(&game, "team3"),
            (600_000, TransferOfferStatus::Pending)
        );
    }

    /// Given a best bid already 30% past what the rival values the player at
    /// When rivals escalate
    /// Then it withdraws however rich it is.
    #[test]
    fn a_rival_never_pays_far_beyond_its_valuation() {
        let mut game = rival_game();
        let valuation =
            suggested_incoming_fee(game.clock.current_date.date_naive(), &game.players[0]);
        bid(&mut game, "team2", valuation);
        bid(&mut game, "team3", valuation * 13 / 10);

        escalate_rival_bids(&mut game);

        assert_eq!(fee_of(&game, "team2").1, TransferOfferStatus::Withdrawn);
    }

    /// Given a single bid
    /// When rivals escalate
    /// Then nothing changes: a bid with no rival has nothing to answer.
    #[test]
    fn a_lone_bid_is_left_alone() {
        let mut game = rival_game();
        bid(&mut game, "team2", 500_000);

        assert_eq!(escalate_rival_bids(&mut game), 0);
        assert_eq!(
            fee_of(&game, "team2"),
            (500_000, TransferOfferStatus::Pending)
        );
    }
}
