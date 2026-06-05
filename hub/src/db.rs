use crate::types::ResourceReport;
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

pub async fn upsert_agent(
    pool: &SqlitePool,
    id: &str,
    url: &str,
    status: &str,
    last_seen: &str,
    last_error: Option<&str>,
    resources: Option<&ResourceReport>,
) -> anyhow::Result<()> {
    let resources_json: Option<String> = match resources {
        Some(r) => Some(serde_json::to_string(r)?),
        None => None,
    };

    sqlx::query(
        r#"
        INSERT INTO agents (
            id, url, status, last_seen, last_error, resources_json
        )
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            url = excluded.url,
            status = excluded.status,
            last_seen = excluded.last_seen,
            last_error = excluded.last_error,
            resources_json = excluded.resources_json;
        "#,
    )
    .bind(id)
    .bind(url)
    .bind(status)
    .bind(last_seen)
    .bind(last_error)
    .bind(resources_json)
    .execute(pool)
    .await?;

    Ok(())
}
