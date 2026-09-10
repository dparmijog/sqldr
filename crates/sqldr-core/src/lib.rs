pub mod driver;
pub mod guard;
pub mod history;
pub mod mysql;
pub mod pagination;
pub use guard::{is_mutating, needs_where_confirmation};
pub use history::{History, HistoryEntry};
pub use pagination::{paginate, DEFAULT_PAGE_SIZE};

pub use driver::{
    Column, ConnConfig, Dialect, Driver, ForeignKey, Plan, Row, Schema, Table, Value,
};
pub use mysql::MySqlDriver;

/// Dispatches to the right [`Driver`] implementation for `cfg.url`'s
/// scheme — the single place a future Postgres/SQLite backend plugs in.
/// Every caller (CLI, TUI wizard, background reconnects) goes through
/// this instead of naming a concrete driver type directly, so adding an
/// engine means adding one match arm here, not hunting down every
/// `MySqlDriver::connect` call site across the TUI crate.
pub async fn connect(cfg: &ConnConfig) -> anyhow::Result<std::sync::Arc<dyn Driver>> {
    let scheme = cfg.url.split("://").next().unwrap_or("");
    match scheme {
        "mysql" => Ok(std::sync::Arc::new(MySqlDriver::connect(cfg).await?)),
        other => anyhow::bail!("unsupported connection scheme '{other}' (only mysql is supported today)"),
    }
}
