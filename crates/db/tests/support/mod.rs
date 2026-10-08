use std::{fs, path::Path};

use sqlx::SqlitePool;

/// Run a database test against the last schema before Plan PR11's cutover.
/// These tests exercise the preserved historical repositories; PR11 behavior
/// is covered separately by `pr11_retirement.rs` against the current schema.
pub async fn migrate_through(pool: &SqlitePool, limit: i64) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let temp = tempfile::tempdir().expect("temporary migration directory");
    let destination = temp.path().join("migrations");
    fs::create_dir_all(&destination).expect("migration directory creates");

    for entry in fs::read_dir(source).expect("migration source reads") {
        let path = entry.expect("migration entry reads").path();
        let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some((version, _)) = stem
            .strip_prefix('V')
            .and_then(|name| name.split_once("__"))
        else {
            continue;
        };
        let version = version.parse::<i64>().expect("migration version parses");
        if version <= limit {
            fs::copy(
                &path,
                destination.join(path.file_name().expect("migration filename")),
            )
            .expect("migration copies");
        }
    }

    db::run_migrations_from(pool, destination)
        .await
        .expect("historical schema migrations apply");
}
