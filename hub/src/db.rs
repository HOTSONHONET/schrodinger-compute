use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};

pub async fn init_db(database_url: &str) -> anyhow::Result<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    // Create sessions
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            url TEXT NOT NULL,
            status TEXT NOT NULL,
            last_seen TIMESTAMP NOT NULL,
            last_error TEXT,
            resources_json TEXT
        );
        "#,
    )
    .execute(&pool)
    .await?;

    Ok(pool)
}
