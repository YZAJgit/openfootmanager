//! Transfer-fee instalments (`Game::transfer_installments`), one row each.
//!
//! The list is small and changes as a whole (a tranche settles, a new deal adds some), so a save
//! replaces the table rather than tracking which rows moved.

use domain::finance::TransferInstallment;
use rusqlite::{Connection, OptionalExtension, params};

const WRITE_ERROR: &str = "be.error.gamePersistence.writeFailed";
const LOAD_ERROR: &str = "be.error.gamePersistence.loadFailed";

/// Replace every stored instalment with `installments`. The caller holds the transaction
/// (`write_game_to_connection`), so a failure leaves the previous rows in place.
pub fn replace_transfer_installments(
    conn: &Connection,
    installments: &[TransferInstallment],
) -> Result<(), String> {
    conn.execute("DELETE FROM transfer_installments", [])
        .map_err(|_| WRITE_ERROR.to_string())?;
    let mut stmt = conn
        .prepare(
            "INSERT INTO transfer_installments
             (id, buyer_team_id, seller_team_id, player_id, amount, due_date, settled,
              created_date)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .map_err(|_| WRITE_ERROR.to_string())?;
    for installment in installments {
        stmt.execute(params![
            installment.id,
            installment.buyer_team_id,
            installment.seller_team_id,
            installment.player_id,
            installment.amount,
            installment.due_date,
            i32::from(installment.settled),
            installment.created_date,
        ])
        .map_err(|error| {
            log::error!(
                "[installment_repo] writing instalment {} failed: {error}",
                installment.id
            );
            WRITE_ERROR.to_string()
        })?;
    }
    Ok(())
}

/// Every stored instalment, in the order it was scheduled. A database older than v068 has no
/// table and reads as none.
pub fn load_transfer_installments(conn: &Connection) -> Result<Vec<TransferInstallment>, String> {
    if !table_exists(conn)? {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, buyer_team_id, seller_team_id, player_id, amount, due_date, settled,
                    created_date
             FROM transfer_installments
             ORDER BY rowid",
        )
        .map_err(|_| LOAD_ERROR.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            let settled: i32 = row.get(6)?;
            Ok(TransferInstallment {
                id: row.get(0)?,
                buyer_team_id: row.get(1)?,
                seller_team_id: row.get(2)?,
                player_id: row.get(3)?,
                amount: row.get(4)?,
                due_date: row.get(5)?,
                settled: settled != 0,
                created_date: row.get(7)?,
            })
        })
        .map_err(|_| LOAD_ERROR.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| LOAD_ERROR.to_string())
}

fn table_exists(conn: &Connection) -> Result<bool, String> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'transfer_installments'",
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

    fn tranche(id: &str, amount: i64, settled: bool) -> TransferInstallment {
        TransferInstallment {
            id: id.to_string(),
            buyer_team_id: "buyer".to_string(),
            seller_team_id: "seller".to_string(),
            player_id: "p1".to_string(),
            amount,
            due_date: "2026-09-01".to_string(),
            settled,
            created_date: "2026-07-01".to_string(),
        }
    }

    /// Given a settled and an unsettled tranche
    /// When they are written and read back
    /// Then both come back unchanged, in the order they were scheduled.
    #[test]
    fn installments_round_trip_in_order() {
        let db = GameDatabase::open_in_memory().unwrap();
        let written = vec![
            tranche("z-1", 2_500_000, true),
            tranche("a-2", 2_500_000, false),
        ];

        replace_transfer_installments(db.conn(), &written).unwrap();

        assert_eq!(load_transfer_installments(db.conn()).unwrap(), written);
    }

    /// Given a stored list of two tranches
    /// When a list of one is written over it
    /// Then only that one remains: a save replaces the table, it does not append to it.
    #[test]
    fn a_second_write_replaces_the_first() {
        let db = GameDatabase::open_in_memory().unwrap();
        replace_transfer_installments(
            db.conn(),
            &[tranche("i-1", 100, false), tranche("i-2", 100, false)],
        )
        .unwrap();

        replace_transfer_installments(db.conn(), &[tranche("i-2", 100, true)]).unwrap();

        assert_eq!(
            load_transfer_installments(db.conn()).unwrap(),
            vec![tranche("i-2", 100, true)]
        );
    }
}
