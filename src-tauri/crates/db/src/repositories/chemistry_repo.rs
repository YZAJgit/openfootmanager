//! Pair chemistry (`Game::pair_chemistry`), one row per pair of team-mates.
//!
//! The list changes after every matchday in many rows at once, so a save replaces the table
//! rather than tracking which rows moved, as `installment_repo` does.

use domain::character::PairChemistry;
use rusqlite::{Connection, OptionalExtension, params};

const WRITE_ERROR: &str = "be.error.gamePersistence.writeFailed";
const LOAD_ERROR: &str = "be.error.gamePersistence.loadFailed";

/// Replace every stored pair with `pairs`. The caller holds the transaction
/// (`write_game_to_connection`), so a failure leaves the previous rows in place.
pub fn replace_pair_chemistry(conn: &Connection, pairs: &[PairChemistry]) -> Result<(), String> {
    conn.execute("DELETE FROM pair_chemistry", [])
        .map_err(|_| WRITE_ERROR.to_string())?;
    let mut stmt = conn
        .prepare(
            "INSERT OR REPLACE INTO pair_chemistry (player_a, player_b, shared_minutes, bond)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(|_| WRITE_ERROR.to_string())?;
    for pair in pairs {
        // Written in canonical order whatever the caller held, so the primary key sees one pair.
        let (player_a, player_b) = PairChemistry::canonical(&pair.player_a, &pair.player_b);
        stmt.execute(params![
            player_a,
            player_b,
            pair.shared_minutes,
            i32::from(pair.bond)
        ])
        .map_err(|error| {
            log::error!("[chemistry_repo] writing pair {player_a}/{player_b} failed: {error}");
            WRITE_ERROR.to_string()
        })?;
    }
    Ok(())
}

/// Every stored pair, ordered by its ids. A database older than v069 has no table and reads as
/// none.
pub fn load_pair_chemistry(conn: &Connection) -> Result<Vec<PairChemistry>, String> {
    if !table_exists(conn)? {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT player_a, player_b, shared_minutes, bond
             FROM pair_chemistry
             ORDER BY player_a, player_b",
        )
        .map_err(|_| LOAD_ERROR.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let player_a: String = row.get(0)?;
            let player_b: String = row.get(1)?;
            let shared_minutes: u32 = row.get(2)?;
            let bond: i64 = row.get(3)?;
            // A bond outside an i8 can only come from a hand-edited save; it is held to the
            // range the type allows rather than failing the load.
            let bond = i8::try_from(bond.clamp(-100, 100)).unwrap_or_default();
            Ok(PairChemistry::new(
                &player_a,
                &player_b,
                shared_minutes,
                bond,
            ))
        })
        .map_err(|_| LOAD_ERROR.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| LOAD_ERROR.to_string())
}

fn table_exists(conn: &Connection) -> Result<bool, String> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'pair_chemistry'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| LOAD_ERROR.to_string())?;
    Ok(exists.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_database::GameDatabase;

    /// Given two pairs with non-default minutes and bonds, one named out of canonical order
    /// When they are written and read back
    /// Then both come back unchanged, in canonical order
    #[test]
    fn pairs_round_trip_in_canonical_order() {
        let db = GameDatabase::open_in_memory().unwrap();
        let written = vec![
            PairChemistry::new("p9", "p2", 540, 17),
            PairChemistry::new("p1", "p3", 90, -4),
        ];

        replace_pair_chemistry(db.conn(), &written).unwrap();

        let read = load_pair_chemistry(db.conn()).unwrap();
        assert_eq!(
            read,
            vec![
                PairChemistry::new("p1", "p3", 90, -4),
                PairChemistry::new("p2", "p9", 540, 17),
            ]
        );
    }

    /// Given a database from before v069, which has no pair_chemistry table
    /// When the pairs are loaded
    /// Then the load succeeds and reads as no pairs
    #[test]
    fn a_database_without_the_table_reads_as_no_pairs() {
        let db = GameDatabase::open_in_memory().unwrap();
        db.conn().execute("DROP TABLE pair_chemistry", []).unwrap();

        assert_eq!(load_pair_chemistry(db.conn()).unwrap(), vec![]);
    }

    /// Given a stored list of two pairs
    /// When a list of one is written over it
    /// Then only that one remains: a save replaces the table, it does not append to it
    #[test]
    fn a_second_write_replaces_the_first() {
        let db = GameDatabase::open_in_memory().unwrap();
        replace_pair_chemistry(
            db.conn(),
            &[
                PairChemistry::new("a", "b", 10, 1),
                PairChemistry::new("c", "d", 20, 2),
            ],
        )
        .unwrap();

        replace_pair_chemistry(db.conn(), &[PairChemistry::new("c", "d", 30, 3)]).unwrap();

        assert_eq!(
            load_pair_chemistry(db.conn()).unwrap(),
            vec![PairChemistry::new("c", "d", 30, 3)]
        );
    }
}
