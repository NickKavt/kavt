//! Database migrations. Each file in `migrations/` is one version, applied once, in order,
//! inside a transaction, and recorded in `schema_migrations`. A file is never changed after a
//! release: a change to the schema is a new file.
//!
//! `0001_base.sql` is idempotent because installations older than this table ran it on every
//! start; it is applied to them once more and then recorded.

use sqlx::{Acquire, PgPool};

/// (version, name, SQL). Append only.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "base", include_str!("../migrations/0001_base.sql")),
    (2, "import and recurring", include_str!("../migrations/0002_import_recurring.sql")),
    (3, "two-factor sign-in", include_str!("../migrations/0003_totp.sql")),
    (4, "rates by day", include_str!("../migrations/0004_rates.sql")),
    (5, "accounts", include_str!("../migrations/0005_accounts.sql")),
    (6, "recurring account", include_str!("../migrations/0006_recurring_account.sql")),
    (7, "savings accounts", include_str!("../migrations/0007_savings_accounts.sql")),
    (8, "currencies", include_str!("../migrations/0008_currencies.sql")),
    (9, "everyday account by default", include_str!("../migrations/0009_pick_regular.sql")),
    (10, "term deposit return", include_str!("../migrations/0010_term_return.sql")),
    (11, "budget language", include_str!("../migrations/0011_budget_lang.sql")),
];

/// Any number; two servers starting on one database wait for each other on it.
const LOCK: i64 = 0x6b61_7674_6275_6467;

pub async fn run(db: &PgPool) -> anyhow::Result<()> {
    let mut conn = db.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)").bind(LOCK).execute(&mut *conn).await?;
    // The base file's "already exists, skipping" notices are expected; don't log them.
    sqlx::raw_sql("SET client_min_messages TO warning").execute(&mut *conn).await?;
    let result = apply(&mut conn).await;
    sqlx::raw_sql("RESET client_min_messages").execute(&mut *conn).await?;
    sqlx::query("SELECT pg_advisory_unlock($1)").bind(LOCK).execute(&mut *conn).await?;
    result
}

async fn apply(conn: &mut sqlx::PgConnection) -> anyhow::Result<()> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
           version BIGINT PRIMARY KEY,
           name TEXT NOT NULL,
           applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
         )",
    )
    .execute(&mut *conn)
    .await?;
    let done: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_migrations").fetch_one(&mut *conn).await?;
    let known = MIGRATIONS.last().map_or(0, |m| m.0);
    anyhow::ensure!(
        done <= known,
        "the database is at schema version {done}, newer than this server knows ({known}); run a newer version of the app"
    );
    for &(version, name, sql) in MIGRATIONS.iter().filter(|m| m.0 > done) {
        let mut tx = conn.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await.map_err(|e| anyhow::anyhow!("migration {version} ({name}): {e}"))?;
        sqlx::query("INSERT INTO schema_migrations (version, name) VALUES ($1, $2)").bind(version).bind(name).execute(&mut *tx).await?;
        tx.commit().await?;
        tracing::info!(version, name, "database migrated");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions_go_up_by_one() {
        for (i, m) in super::MIGRATIONS.iter().enumerate() {
            assert_eq!(m.0, i as i64 + 1, "{}", m.1);
        }
    }
}
