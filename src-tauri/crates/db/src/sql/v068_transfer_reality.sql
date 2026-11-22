-- Transfer reality (M4).
--
-- A player's release clause, and the terms of the loan he is on: whether his parent club may
-- recall him early and the share of minutes the borrower promised. NULL / 0 for every row written
-- before the columns, which reads as no clause and a loan that cannot be recalled.
ALTER TABLE players ADD COLUMN release_clause INTEGER;
ALTER TABLE players ADD COLUMN loan_recallable INTEGER NOT NULL DEFAULT 0;
ALTER TABLE players ADD COLUMN loan_guaranteed_minutes_pct INTEGER;

-- Transfer fees paid in tranches (`Game::transfer_installments`). A tranche is settled through the
-- cash journal once it falls due; `settled` records that it has been.
CREATE TABLE transfer_installments (
    id TEXT PRIMARY KEY NOT NULL,
    buyer_team_id TEXT NOT NULL,
    seller_team_id TEXT NOT NULL,
    player_id TEXT NOT NULL,
    amount INTEGER NOT NULL,
    due_date TEXT NOT NULL,
    settled INTEGER NOT NULL DEFAULT 0,
    created_date TEXT NOT NULL
);
