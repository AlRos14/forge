use crate::Result;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    SqlitePool,
};
use std::{str::FromStr, time::Duration};

pub async fn create_sqlite_pool(database_url: &str) -> Result<SqlitePool> {
    let max_connections = if database_url.contains(":memory:") {
        1
    } else {
        5
    };
    let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(30))
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("PRAGMA journal_mode = WAL")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("PRAGMA busy_timeout = 30000")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await?;

    Ok(pool)
}
