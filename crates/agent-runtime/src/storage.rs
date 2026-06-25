use crate::session::{Message, Session};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let pool = SqlitePool::connect(url).await?;
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
        let message = Message {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: Utc::now(),
        };

        sqlx::query(
            "INSERT INTO messages (id, session_id, role, content, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&message.id)
        .bind(&message.session_id)
        .bind(&message.role)
        .bind(&message.content)
        .bind(message.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;

        Ok(message)
    }

    pub async fn list_messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        let rows = sqlx::query(
            "SELECT id, session_id, role, content, created_at FROM messages WHERE session_id = ? ORDER BY created_at ASC",
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
}
