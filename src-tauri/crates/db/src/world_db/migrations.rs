//! The world database's own migration set.
//!
//! `rusqlite_migration` keys a database's schema version off its `user_version` and the count of
//! applied scripts, so a second, independent set can live beside the career save's `v001..v067`
//! set as long as it is applied to a different file. It is: `world.db` is never opened through
//! the career `GameDatabase`.
//!
//! Files are named `wNNN_description.sql` and live in `src/world_sql`, not `src/sql`: that
//! directory is the career save's, and its tests require every `.sql` file there to be a
//! registered `vNNN` migration.

use rusqlite_migration::{M, Migrations};

/// Number of world migrations defined.
pub const WORLD_MIGRATION_COUNT: usize = WORLD_MIGRATIONS.len();

/// Pinned like the career save's count: adding a migration fails the build until this is raised,
/// so *lowering* it is a deliberate act. Lowering it against a file written by a newer build
/// would leave a world at a version whose columns this build does not create.
const EXPECTED_WORLD_MIGRATION_COUNT: usize = 2;

const _: () = assert!(
    WORLD_MIGRATION_COUNT == EXPECTED_WORLD_MIGRATION_COUNT,
    "the world migration count changed. If you added a wNNN migration, raise \
     EXPECTED_WORLD_MIGRATION_COUNT to match. If you removed one, put it back: a world.db written \
     by a newer build reports a higher user_version and expects columns this build never creates."
);

/// Every world migration, in application order. Append only — the order is the schema version.
#[rustfmt::skip]
const WORLD_MIGRATIONS: &[(&str, &str)] = &[
    // W1: Clubs, players, season statistics, membership periods, identity staging and FTS5 search
    ("w001_world_schema.sql", include_str!("../world_sql/w001_world_schema.sql")),
    // W2: Competitions, competition membership by season and the sync-run ledger
    ("w002_competitions_and_sync.sql", include_str!("../world_sql/w002_competitions_and_sync.sql")),
];

/// The world migration set, ready to apply to a `world.db` connection.
pub fn world_migrations() -> Migrations<'static> {
    Migrations::new(WORLD_MIGRATIONS.iter().map(|(_, sql)| M::up(sql)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The version a `wNNN_description.sql` file name claims, or `None` if it is not that shape.
    fn migration_version(name: &str) -> Option<usize> {
        let (digits, description) = name
            .strip_suffix(".sql")?
            .strip_prefix('w')?
            .split_once('_')?;
        if digits.len() != 3
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
            || description.is_empty()
        {
            return None;
        }
        digits.parse::<usize>().ok()
    }

    fn sql_files_on_disk() -> Vec<(usize, String)> {
        let sql_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/world_sql");
        let mut files: Vec<(usize, String)> = std::fs::read_dir(&sql_dir)
            .expect("src/world_sql should be readable")
            .map(|entry| entry.expect("readable directory entry").file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".sql"))
            .map(|name| {
                let version = migration_version(&name).unwrap_or_else(|| {
                    panic!(
                        "world migration `{name}` does not follow the wNNN_description.sql naming"
                    )
                });
                (version, name)
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn the_world_migration_set_is_valid() {
        world_migrations()
            .validate()
            .expect("the world migrations should be valid");
    }

    #[test]
    fn every_world_sql_file_is_registered_in_order() {
        let registered: Vec<String> = WORLD_MIGRATIONS
            .iter()
            .map(|(name, _)| String::from(*name))
            .collect();
        let on_disk: Vec<String> = sql_files_on_disk()
            .into_iter()
            .map(|(_, name)| name)
            .collect();

        assert_eq!(
            registered, on_disk,
            "src/world_sql and WORLD_MIGRATIONS disagree. A file with no registration never runs; \
             a registration out of order changes what a given user_version means."
        );
    }

    #[test]
    fn each_world_registration_embeds_the_file_it_names() {
        let sql_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/world_sql");
        for (name, embedded) in WORLD_MIGRATIONS {
            let on_disk = std::fs::read_to_string(sql_dir.join(name))
                .unwrap_or_else(|error| panic!("migration `{name}` is unreadable: {error}"));
            assert_eq!(
                *embedded, on_disk,
                "`{name}` does not embed src/world_sql/{name}"
            );
        }
    }

    #[test]
    fn world_migration_versions_are_contiguous_from_one() {
        for (index, (version, name)) in sql_files_on_disk().iter().enumerate() {
            assert_eq!(
                *version,
                index + 1,
                "expected w{:03} at this position but found `{name}`",
                index + 1
            );
        }
    }
}
