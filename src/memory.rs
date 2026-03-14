use anyhow::Result;
use rusqlite::params;

use crate::db::Database;

pub struct AgentMemory {
    facts: Vec<MemoryEntry>,
    last_refresh: std::time::Instant,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
    pub kind: MemoryKind,
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryKind {
    UserSet,
    Inferred,
    ProjectFact,
    CommandPattern,
}

impl MemoryKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::UserSet => "user_set",
            Self::Inferred => "inferred",
            Self::ProjectFact => "project_fact",
            Self::CommandPattern => "command_pattern",
        }
    }

    pub fn as_str_pub(&self) -> &'static str {
        self.as_str()
    }

    fn from_str(s: &str) -> Self {
        match s {
            "user_set" => Self::UserSet,
            "inferred" => Self::Inferred,
            "project_fact" => Self::ProjectFact,
            "command_pattern" => Self::CommandPattern,
            _ => Self::Inferred,
        }
    }
}

impl AgentMemory {
    pub fn new() -> Self {
        Self {
            facts: Vec::new(),
            last_refresh: std::time::Instant::now()
                - std::time::Duration::from_secs(300),
        }
    }

    pub fn init_table(db: &Database) -> Result<()> {
        db.connection().execute(
            "CREATE TABLE IF NOT EXISTS agent_memory (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                key TEXT NOT NULL UNIQUE,
                value TEXT NOT NULL,
                kind TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            )",
            [],
        )?;
        Ok(())
    }

    pub fn load(&mut self, db: &Database) {
        if self.last_refresh.elapsed().as_secs() < 30 && !self.facts.is_empty() {
            return;
        }
        self.last_refresh = std::time::Instant::now();

        let conn = db.connection();
        if let Ok(mut stmt) = conn.prepare("SELECT key, value, kind, timestamp FROM agent_memory ORDER BY timestamp DESC LIMIT 100") {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok(MemoryEntry {
                    key: row.get(0)?,
                    value: row.get(1)?,
                    kind: MemoryKind::from_str(&row.get::<_, String>(2)?),
                    timestamp: row.get(3)?,
                })
            }) {
                self.facts = rows.filter_map(|r| r.ok()).collect();
            }
        }
    }

    pub fn remember(db: &Database, key: &str, value: &str, kind: MemoryKind) -> Result<()> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        db.connection().execute(
            "INSERT OR REPLACE INTO agent_memory (key, value, kind, timestamp) VALUES (?1, ?2, ?3, ?4)",
            params![key, value, kind.as_str(), timestamp],
        )?;
        Ok(())
    }

    pub fn forget(db: &Database, key: &str) -> Result<bool> {
        let count = db.connection().execute(
            "DELETE FROM agent_memory WHERE key = ?1",
            params![key],
        )?;
        Ok(count > 0)
    }

    pub fn recall(db: &Database, key: &str) -> Option<String> {
        db.connection()
            .query_row(
                "SELECT value FROM agent_memory WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .ok()
    }

    pub fn context_block(&self) -> String {
        if self.facts.is_empty() {
            return String::new();
        }

        let mut block = String::from("Agent memory (persistent facts from previous sessions):\n");
        for entry in &self.facts {
            block.push_str(&format!("- {}: {}\n", entry.key, entry.value));
        }
        block
    }

    pub fn all_entries(&self) -> &[MemoryEntry] {
        &self.facts
    }
}
