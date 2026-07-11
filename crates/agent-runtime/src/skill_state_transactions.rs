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
        Err(error) => match transaction.rollback().await {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(add_rollback_failure_context(error, rollback_error)),
        },
    }
}

fn add_rollback_failure_context(
    error: anyhow::Error,
    rollback_error: sqlx::Error,
) -> anyhow::Error {
    error.context(format!("transaction rollback failed: {rollback_error}"))
}

#[cfg(test)]
mod tests {
    use super::add_rollback_failure_context;

    #[test]
    fn rollback_failure_context_preserves_original_business_error() {
        let error = add_rollback_failure_context(
            anyhow::anyhow!("snapshot must be active"),
            sqlx::Error::Protocol("rollback failed".into()),
        );
        let message = format!("{error:#}");

        assert!(message.contains("snapshot must be active"), "{message}");
        assert!(message.contains("rollback failed"), "{message}");
        assert_eq!(error.root_cause().to_string(), "snapshot must be active");
    }
}
