-- Character and chemistry (M5).
--
-- A player's character profile and a team's familiarity with its formation, each as JSON. NULL for
-- every row written before the columns, which reads as the neutral profile (every trait 50) and a
-- side that has not yet built familiarity with any shape.
ALTER TABLE players ADD COLUMN character_json TEXT;
ALTER TABLE teams ADD COLUMN tactical_familiarity_json TEXT;

-- The on-pitch understanding between two team-mates (`Game::pair_chemistry`), one row per pair in
-- canonical order (player_a <= player_b): minutes played together and a bond in -100..=100.
CREATE TABLE pair_chemistry (
    player_a TEXT NOT NULL,
    player_b TEXT NOT NULL,
    shared_minutes INTEGER NOT NULL DEFAULT 0,
    bond INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (player_a, player_b)
);
