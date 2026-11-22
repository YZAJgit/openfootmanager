//! Calling a loanee back before his loan runs out.
//!
//! Only the parent club can recall a player, and only from a loan agreed as recallable
//! (`Player::loan_recallable`). A recall ends the loan as its natural end does
//! (`loans::return_loanee_to_parent`): the player goes home, the loan and its terms are cleared
//! and a `LoanReturn` is written into his history.

use super::loans::return_loanee_to_parent;
use super::*;

/// Why a recall was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoanRecallRefusal {
    PlayerNotFound,
    NotOnLoan,
    /// Only the club that lent the player can call him back.
    NotParentClub,
    /// The loan was agreed without a recall clause.
    NotRecallable,
}

/// Bring `player_id` home from loan early, at the request of `requesting_team_id`.
pub fn recall_loan(
    game: &mut Game,
    player_id: &str,
    requesting_team_id: &str,
) -> Result<(), LoanRecallRefusal> {
    let player = game
        .players
        .iter()
        .find(|player| player.id == player_id)
        .ok_or(LoanRecallRefusal::PlayerNotFound)?;
    let loan = player
        .active_loan
        .as_ref()
        .ok_or(LoanRecallRefusal::NotOnLoan)?;
    if loan.parent_team_id != requesting_team_id {
        return Err(LoanRecallRefusal::NotParentClub);
    }
    if !player.loan_recallable {
        return Err(LoanRecallRefusal::NotRecallable);
    }
    return_loanee_to_parent(game, player_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's (team1) player out on loan at team2 until the summer.
    fn loaned_out(recallable: bool) -> Game {
        let mut game = super::super::tests::make_game();
        let player = &mut game.players[0];
        player.team_id = Some("team2".to_string());
        player.active_loan = Some(ActiveLoan {
            parent_team_id: "team1".to_string(),
            loan_team_id: "team2".to_string(),
            start_date: "2025-08-01".to_string(),
            end_date: "2026-06-30".to_string(),
            wage_contribution_pct: 50,
            buy_option_fee: None,
            loan_start_minutes: 0,
            loan_start_appearances: 0,
            development_reported_minutes: 0,
            development_reported_appearances: 0,
        });
        player.loan_recallable = recallable;
        player.loan_guaranteed_minutes_pct = Some(60);
        game
    }

    /// Given a recallable loan
    /// When the parent club recalls the player
    /// Then he is back at the parent club, off loan, his loan terms are cleared and his history
    ///      records the return.
    #[test]
    fn a_recallable_loanee_comes_home() {
        let mut game = loaned_out(true);

        assert_eq!(recall_loan(&mut game, "player-award", "team1"), Ok(()));

        let player = &game.players[0];
        assert_eq!(player.team_id.as_deref(), Some("team1"));
        assert!(player.active_loan.is_none());
        assert!(!player.loan_recallable);
        assert_eq!(player.loan_guaranteed_minutes_pct, None);
        let last = player.movement_history.iter().last().unwrap();
        assert_eq!(last.kind, PlayerMovementKind::LoanReturn);
        assert_eq!(last.from_team_id.as_deref(), Some("team2"));
        assert_eq!(last.to_team_id.as_deref(), Some("team1"));
    }

    /// Given a loan agreed without a recall clause
    /// When the parent club tries to recall the player
    /// Then it is refused and the player stays on loan.
    #[test]
    fn a_loan_without_a_recall_clause_cannot_be_ended_early() {
        let mut game = loaned_out(false);

        assert_eq!(
            recall_loan(&mut game, "player-award", "team1"),
            Err(LoanRecallRefusal::NotRecallable)
        );
        assert_eq!(game.players[0].team_id.as_deref(), Some("team2"));
        assert!(game.players[0].active_loan.is_some());
    }

    /// Given a recallable loan
    /// When the borrowing club, or anyone but the parent, tries to recall him
    /// Then it is refused.
    #[test]
    fn only_the_parent_club_can_recall() {
        let mut game = loaned_out(true);

        assert_eq!(
            recall_loan(&mut game, "player-award", "team2"),
            Err(LoanRecallRefusal::NotParentClub)
        );
        assert!(game.players[0].active_loan.is_some());
    }

    /// Given a player who is not on loan, and one who does not exist
    /// When a recall is asked for
    /// Then each is refused for its own reason.
    #[test]
    fn there_is_nothing_to_recall_without_a_loan() {
        let mut game = super::super::tests::make_game();

        assert_eq!(
            recall_loan(&mut game, "player-award", "team1"),
            Err(LoanRecallRefusal::NotOnLoan)
        );
        assert_eq!(
            recall_loan(&mut game, "nobody", "team1"),
            Err(LoanRecallRefusal::PlayerNotFound)
        );
    }
}
