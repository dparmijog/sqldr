//! `~/.config/sqldr/config.toml` loading and per-connection password
//! resolution via the OS keyring.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "sqldr";

#[derive(Debug, Clone, Deserialize)]
pub struct ConnEntry {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    #[serde(rename = "connections", default)]
    pub connections: Vec<ConnEntry>,
}

pub fn config_path() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "sqldr")
        .context("could not determine config directory")?;
    Ok(dirs.config_dir().join("config.toml"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Config::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

impl Config {
    pub fn find(&self, name: &str) -> Result<&ConnEntry> {
        self.connections
            .iter()
            .find(|c| c.name == name)
            .with_context(|| format!("no connection named '{name}' in {}", config_path().unwrap_or_default().display()))
    }
}

/// Looks up a stored password for `conn_name` in the OS keyring.
pub fn get_password(conn_name: &str) -> Result<Option<String>> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, conn_name)?;
    match entry.get_password() {
        Ok(pw) => Ok(Some(pw)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Stores a password for `conn_name` in the OS keyring.
pub fn set_password(conn_name: &str, password: &str) -> Result<()> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, conn_name)?;
    entry.set_password(password)?;
    Ok(())
}

/// Resolves a full connection URL by injecting a keyring-stored password
/// when the configured URL doesn't already carry one.
pub fn resolve_url(entry: &ConnEntry) -> Result<String> {
    let mut url = url::Url::parse(&entry.url)
        .with_context(|| format!("invalid connection url for '{}'", entry.name))?;

    if url.password().is_none() {
        if let Some(pw) = get_password(&entry.name)? {
            url.set_password(Some(&pw))
                .map_err(|_| anyhow::anyhow!("could not set password on url for '{}'", entry.name))?;
        }
    }

    Ok(url.into())
}
