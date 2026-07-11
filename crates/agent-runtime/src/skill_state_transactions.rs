use sqlx::{Sqlite, SqlitePool, Transaction};

pub(crate) async fn begin_immediate(
    pool: &SqlitePool,
) -> anyhow::Result<Transaction<'static, Sqlite>> {
    Ok(pool.begin_with("BEGIN IMMEDIATE").await?)
}

pub(crate) async fn finish<T>(
    transaction: Transaction<'static, Sqlite>,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    match result {
        Ok(value) => {
            transaction.commit().await?;
            Ok(value)
        }
        Err(error) => {
            transaction.rollback().await?;
            Err(error)
        }
    }
}
