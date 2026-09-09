//! Recently-viewed and favorited tables, shown pinned at the top of the
//! sidebar tree (`Ctrl+O`-independent — always visible) so a table you use
//! often doesn't need re-navigating the connection/database tree or the
//! `/` search every time.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// How many most-recently-viewed tables to remember (beyond favorites,
/// which have no cap).
const MAX_RECENT: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableRef {
    pub conn: String,
    pub db: String,
    pub table: String,
}

impl TableRef {
    pub fn label(&self) -> String {
        format!("{}/{}/{}", self.conn, self.db, self.table)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RecentTables {
    #[serde(default)]
    pub recent: Vec<TableRef>,
    #[serde(default)]
    pub favorites: Vec<TableRef>,
}

impl RecentTables {
    /// Loads from `path`; a missing or corrupt file yields empty defaults
    /// rather than failing startup over a convenience feature.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        Ok(())
    }

    /// Records `entry` as just-viewed: moves it to the front, dedups, and
    /// caps the list so it stays a genuinely "recent" handful.
    pub fn touch(&mut self, entry: TableRef) {
        self.recent.retain(|e| *e != entry);
        self.recent.insert(0, entry);
        self.recent.truncate(MAX_RECENT);
    }

    /// Toggles favorite status for `entry`; returns `true` if it's now a
    /// favorite, `false` if it was just removed.
    pub fn toggle_favorite(&mut self, entry: TableRef) -> bool {
        if let Some(pos) = self.favorites.iter().position(|e| *e == entry) {
            self.favorites.remove(pos);
            false
        } else {
            self.favorites.push(entry);
            true
        }
    }

    pub fn is_favorite(&self, entry: &TableRef) -> bool {
        self.favorites.iter().any(|e| e == entry)
    }

    /// Recent entries that aren't already favorited — favorites get their
    /// own section, so showing them twice would be noise.
    pub fn recent_excluding_favorites(&self) -> Vec<&TableRef> {
        self.recent.iter().filter(|e| !self.is_favorite(e)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str) -> TableRef {
        TableRef { conn: "a".into(), db: "d".into(), table: name.into() }
    }

    #[test]
    fn touch_moves_existing_entry_to_front_without_duplicating() {
        let mut recents = RecentTables::default();
        recents.touch(table("t1"));
        recents.touch(table("t2"));
        recents.touch(table("t1"));
        assert_eq!(recents.recent.len(), 2);
        assert_eq!(recents.recent[0].table, "t1");
        assert_eq!(recents.recent[1].table, "t2");
    }

    #[test]
    fn touch_caps_at_max_recent() {
        let mut recents = RecentTables::default();
        for i in 0..(MAX_RECENT + 5) {
            recents.touch(table(&format!("t{i}")));
        }
        assert_eq!(recents.recent.len(), MAX_RECENT);
        assert_eq!(recents.recent[0].table, format!("t{}", MAX_RECENT + 4));
    }

    #[test]
    fn toggle_favorite_adds_then_removes() {
        let mut recents = RecentTables::default();
        let entry = table("t");
        assert!(recents.toggle_favorite(entry.clone()));
        assert!(recents.is_favorite(&entry));
        assert!(!recents.toggle_favorite(entry.clone()));
        assert!(!recents.is_favorite(&entry));
    }

    #[test]
    fn recent_excluding_favorites_hides_favorited_entries() {
        let mut recents = RecentTables::default();
        let entry = table("t");
        recents.touch(entry.clone());
        recents.toggle_favorite(entry);
        assert!(recents.recent_excluding_favorites().is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir()
            .join(format!("sqldr-recents-test-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("recent_tables.json");
        let mut recents = RecentTables::default();
        recents.touch(table("t"));
        recents.toggle_favorite(table("fav"));
        recents.save(&path).unwrap();

        let loaded = RecentTables::load(&path);
        assert_eq!(loaded.recent, recents.recent);
        assert_eq!(loaded.favorites, recents.favorites);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_yields_empty_defaults() {
        let path = std::env::temp_dir().join("sqldr-recents-does-not-exist.json");
        let loaded = RecentTables::load(&path);
        assert!(loaded.recent.is_empty());
        assert!(loaded.favorites.is_empty());
    }
}
