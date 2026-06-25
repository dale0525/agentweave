use crate::session::{Message, Session};
use chrono::{DateTime, Duration, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, Row, Sqlite, SqlitePool};
use std::str::FromStr;
use uuid::Uuid;

#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::from_str(url)?.foreign_keys(true);
        let pool = SqlitePoolOptions::new().connect_with(options).await?;
        let storage = Self { pool };
        storage.migrate().await?;
        Ok(storage)
    }

    async fn migrate(&self) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
              id TEXT PRIMARY KEY,
              title TEXT NOT NULL,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
              id TEXT PRIMARY KEY,
              session_id TEXT NOT NULL,
              role TEXT NOT NULL,
              content TEXT NOT NULL,
              created_at TEXT NOT NULL,
              FOREIGN KEY(session_id) REFERENCES sessions(id)
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn create_session(&self, title: &str) -> anyhow::Result<Session> {
        let now = Utc::now();
        let session = Session {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            created_at: now,
            updated_at: now,
        };

        sqlx::query("INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(&session.id)
            .bind(&session.title)
            .bind(session.created_at.to_rfc3339())
            .bind(session.updated_at.to_rfc3339())
            .execute(&self.pool)
            .await?;

        Ok(session)
    }

    pub async fn append_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
    ) -> anyhow::Result<Message> {
        let message = build_message(session_id, role, content, Utc::now());

        insert_message(&self.pool, &message).await?;

        Ok(message)
    }

    pub async fn append_turn(
        &self,
        session_id: &str,
        user_content: &str,
        assistant_content: &str,
    ) -> anyhow::Result<(Message, Message)> {
        let user_created_at = Utc::now();
        let assistant_created_at = user_created_at + Duration::microseconds(1);
        let user_message = build_message(session_id, "user", user_content, user_created_at);
        let assistant_message = build_message(
            session_id,
            "assistant",
            assistant_content,
            assistant_created_at,
        );
        let mut tx = self.pool.begin().await?;

        insert_message(&mut *tx, &user_message).await?;
        insert_message(&mut *tx, &assistant_message).await?;
        tx.commit().await?;

        Ok((user_message, assistant_message))
    }

    pub async fn session_exists(&self, session_id: &str) -> anyhow::Result<bool> {
        let exists: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?)")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await?;

        Ok(exists != 0)
    }

    pub async fn list_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        let rows = sqlx::query(
            "SELECT id, session_id, role, content, created_at FROM messages WHERE session_id = ? ORDER BY created_at ASC, id ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            let created_at: String = row.try_get("created_at")?;
            messages.push(Message {
                id: row.try_get("id")?,
                session_id: row.try_get("session_id")?,
                role: row.try_get("role")?,
                content: row.try_get("content")?,
                created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
            });
        }

        Ok(messages)
    }
}

fn build_message(
    session_id: &str,
    role: &str,
    content: &str,
    created_at: DateTime<Utc>,
) -> Message {
    Message {
        id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        role: role.to_string(),
        content: content.to_string(),
        created_at,
    }
}

async fn insert_message<'a, E>(executor: E, message: &Message) -> anyhow::Result<()>
where
    E: Executor<'a, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&message.id)
    .bind(&message.session_id)
    .bind(&message.role)
    .bind(&message.content)
    .bind(message.created_at.to_rfc3339())
    .execute(executor)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stores_and_lists_messages() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let session = storage.create_session("Test").await.unwrap();

        storage
            .append_message(&session.id, "user", "hello")
            .await
            .unwrap();

        let messages = storage.list_messages(&session.id).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
    }

    #[tokio::test]
    async fn rejects_messages_for_missing_sessions() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();

        let result = storage
            .append_message("missing-session", "user", "hello")
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn reports_session_existence() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let session = storage.create_session("Test").await.unwrap();

        assert!(storage.session_exists(&session.id).await.unwrap());
        assert!(!storage.session_exists("missing-session").await.unwrap());
    }

    #[tokio::test]
    async fn appends_user_and_assistant_turn_messages() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let session = storage.create_session("Test").await.unwrap();

        let (user_message, assistant_message) = storage
            .append_turn(&session.id, "hello", "MVP agent received: hello")
            .await
            .unwrap();

        assert_eq!(user_message.role, "user");
        assert_eq!(user_message.content, "hello");
        assert_eq!(assistant_message.role, "assistant");
        assert_eq!(assistant_message.content, "MVP agent received: hello");

        let messages = storage.list_messages(&session.id).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, user_message.id);
        assert_eq!(messages[1].id, assistant_message.id);
    }

    #[tokio::test]
    async fn append_turn_rolls_back_when_assistant_insert_fails() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let session = storage.create_session("Test").await.unwrap();
        sqlx::query(
            r#"
            CREATE TRIGGER fail_assistant_insert
            BEFORE INSERT ON messages
            WHEN NEW.role = 'assistant'
            BEGIN
                SELECT RAISE(ABORT, 'assistant insert failed');
            END;
            "#,
        )
        .execute(&storage.pool)
        .await
        .unwrap();

        let result = storage
            .append_turn(&session.id, "hello", "MVP agent received: hello")
            .await;

        assert!(result.is_err());
        let messages = storage.list_messages(&session.id).await.unwrap();
        assert!(messages.is_empty());
    }
}
