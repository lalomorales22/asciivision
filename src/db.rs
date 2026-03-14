use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::PathBuf;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

impl Database {
    pub fn new() -> Result<Self> {
        let path = Self::db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("failed to open database at {}", path.display()))?;

        let db = Self { conn };
        db.init()?;
        Ok(db)
    }

    fn db_path() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;
        Ok(PathBuf::from(home).join(".config/asciivision/conversations.db"))
    }

    fn init(&self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL,
                role TEXT NOT NULL,
                kind TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(())
    }

    pub fn save_message(
        &self,
        provider: &str,
        role: &str,
        kind: &str,
        content: &str,
    ) -> Result<()> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        self.conn.execute(
            "INSERT INTO messages (provider, role, kind, content, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![provider, role, kind, content, timestamp],
        )?;
        Ok(())
    }
}
