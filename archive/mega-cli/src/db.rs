use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::PathBuf;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Message {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new() -> Result<Self> {
        let db_path = Self::get_db_path()?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open database at {:?}", db_path))?;

        let db = Self { conn };
        db.init_tables()?;
        Ok(db)
    }

    fn get_db_path() -> Result<PathBuf> {
        let home = std::env::var("HOME")
            .context("HOME environment variable not set")?;
        Ok(PathBuf::from(home).join(".config/mega-cli/conversations.db"))
    }

    fn init_tables(&self) -> Result<()> {
        // Create tables for each AI provider
        let providers = ["claude", "grok", "gpt", "gemini"];

        for provider in &providers {
            let table_name = format!("{}_messages", provider);
            let create_sql = format!(
                "CREATE TABLE IF NOT EXISTS {} (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    timestamp INTEGER NOT NULL
                )",
                table_name
            );
            self.conn.execute(&create_sql, [])?;
        }

        Ok(())
    }

    pub fn save_message(&self, provider: &str, role: &str, content: &str) -> Result<i64> {
        let table_name = format!("{}_messages", provider.to_lowercase());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        let insert_sql = format!(
            "INSERT INTO {} (role, content, timestamp) VALUES (?1, ?2, ?3)",
            table_name
        );

        self.conn.execute(&insert_sql, params![role, content, timestamp])?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(dead_code)]
    pub fn get_messages(&self, provider: &str, limit: Option<usize>) -> Result<Vec<Message>> {
        let table_name = format!("{}_messages", provider.to_lowercase());
        let (query, order) = if limit.is_some() {
            (
                format!("SELECT id, role, content, timestamp FROM {} ORDER BY id DESC LIMIT ?1", table_name),
                true,
            )
        } else {
            (
                format!("SELECT id, role, content, timestamp FROM {} ORDER BY id ASC", table_name),
                false,
            )
        };

        let mut stmt = self.conn.prepare(&query)?;
        let message_iter = if let Some(lim) = limit {
            stmt.query_map(params![lim], Self::map_message)?
        } else {
            stmt.query_map([], Self::map_message)?
        };

        let mut result: Vec<Message> = message_iter.collect::<Result<_, _>>()?;

        // If we used LIMIT with DESC, reverse to get chronological order
        if order {
            result.reverse();
        }

        Ok(result)
    }

    fn map_message(row: &rusqlite::Row) -> rusqlite::Result<Message> {
        Ok(Message {
            id: row.get(0)?,
            role: row.get(1)?,
            content: row.get(2)?,
            timestamp: row.get(3)?,
        })
    }

    pub fn clear_history(&self, provider: &str) -> Result<()> {
        let table_name = format!("{}_messages", provider.to_lowercase());
        let delete_sql = format!("DELETE FROM {}", table_name);
        self.conn.execute(&delete_sql, [])?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_conversation_count(&self, provider: &str) -> Result<usize> {
        let table_name = format!("{}_messages", provider.to_lowercase());
        let count_sql = format!("SELECT COUNT(*) FROM {}", table_name);
        let count: usize = self.conn.query_row(&count_sql, [], |row| row.get(0))?;
        Ok(count)
    }
}
