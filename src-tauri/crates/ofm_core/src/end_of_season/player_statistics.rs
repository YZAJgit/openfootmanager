use super::is_regenerated_at_rollover;
use crate::game::Game;
use std::collections::HashSet;

/// A global rollover starts a new competitive season, but foreign league editions can still
/// be in play. Their per-competition records last until those editions are actually renewed.
pub(super) fn reset_for_new_season(game: &mut Game) {
    let retained: HashSet<_> = game
        .competitions_in_play()
        .iter()
        .filter(|competition| !is_regenerated_at_rollover(competition))
        .map(|competition| competition.id.clone())
        .collect();
    for player in &mut game.players {
        player.reset_season_totals();
        // Loan baselines are offsets into the season totals just zeroed; left alone they would
        // exceed the new total and read as a breach of the minutes promise.
        if let Some(loan) = player.active_loan.as_mut() {
            loan.loan_start_minutes = 0;
            loan.loan_start_appearances = 0;
        }
        player
            .league_stats_by_competition
            .retain(|id, _| retained.contains(id));
    }
}
