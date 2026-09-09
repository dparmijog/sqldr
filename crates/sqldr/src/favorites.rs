//! Favorited databases, pinned at the top of the sidebar tree so a
//! database you use often doesn't need re-navigating the connection tree
//! or the `/` search every time.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbRef {
    pub conn: String,
    pub db: String,
}

impl DbRef {
    pub fn label(&self) -> String {
        format!("{}/{}", self.conn, self.db)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Favorites {
    #[serde(default)]
    pub favorites: Vec<DbRef>,
}

impl Favorites {
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

    /// Toggles favorite status for `entry`; returns `true` if it's now a
    /// favorite, `false` if it was just removed.
    pub fn toggle(&mut self, entry: DbRef) -> bool {
        if let Some(pos) = self.favorites.iter().position(|e| *e == entry) {
            self.favorites.remove(pos);
            false
        } else {
            self.favorites.push(entry);
            true
        }
    }

    pub fn is_favorite(&self, entry: &DbRef) -> bool {
        self.favorites.iter().any(|e| e == entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(name: &str) -> DbRef {
        DbRef { conn: "a".into(), db: name.into() }
    }

    #[test]
    fn toggle_adds_then_removes() {
        let mut favorites = Favorites::default();
        let entry = db("d");
        assert!(favorites.toggle(entry.clone()));
        assert!(favorites.is_favorite(&entry));
        assert!(!favorites.toggle(entry.clone()));
        assert!(!favorites.is_favorite(&entry));
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("sqldr-favorites-test-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("favorites.json");
        let mut favorites = Favorites::default();
        favorites.toggle(db("fav"));
        favorites.save(&path).unwrap();

        let loaded = Favorites::load(&path);
        assert_eq!(loaded.favorites, favorites.favorites);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_yields_empty_defaults() {
        let path = std::env::temp_dir().join("sqldr-favorites-does-not-exist.json");
        let loaded = Favorites::load(&path);
        assert!(loaded.favorites.is_empty());
    }
}
