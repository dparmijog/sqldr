//! Per-connection query history, persisted as newline-delimited JSON so
//! multi-line SQL round-trips safely.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub sql: String,
    /// Unix timestamp (seconds) the query was run at.
    pub at: u64,
}

/// A connection's query history, backed by an append-only JSONL file. Most
/// recent entry last; callers that want most-recent-first should reverse.
pub struct History {
    path: PathBuf,
    entries: Vec<HistoryEntry>,
}

impl History {
    /// Loads history from `path`, tolerating a missing file (empty history)
    /// and skipping any unparseable lines rather than failing outright.
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let entries = if path.exists() {
            let file = std::fs::File::open(&path)?;
            BufReader::new(file)
                .lines()
                .map_while(Result::ok)
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<HistoryEntry>(&line).ok())
                .collect()
        } else {
            Vec::new()
        };
        Ok(History { path, entries })
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Appends `sql` unless it's identical to the most recent entry
    /// (avoids flooding history with repeated re-runs of the same query).
    pub fn push(&mut self, sql: &str) -> anyhow::Result<()> {
        if self.entries.last().is_some_and(|e| e.sql == sql) {
            return Ok(());
        }
        let entry = HistoryEntry {
            sql: sql.to_string(),
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        self.entries.push(entry);
        Ok(())
    }

    /// Path this history is persisted to, exposed for diagnostics.
    pub fn path(&self) -> &Path {
        &self.path
    }
}
