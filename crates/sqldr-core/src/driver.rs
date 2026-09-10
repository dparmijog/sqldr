//! Core abstractions shared by every database backend.
//!
//! `sqldr-core` knows nothing about terminals or UI: it only exposes the
//! [`Driver`] trait plus the value/schema types every implementation speaks.

use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio_util::sync::CancellationToken;

/// Structured connection-failure classification. Every `Driver` method
/// besides `connect` still returns a plain `anyhow::Result` — connect is
/// the one place where distinguishing *why* a connection failed (wrong
/// password vs. server unreachable vs. typo'd database name) is
/// genuinely actionable for a caller, and its `Display` text (used by
/// every existing `.to_string()` call site in the TUI) is already an
/// improvement over the raw driver error even for callers that never
/// match on the variant.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    #[error("could not reach the server: {0}")]
    ConnectionRefused(String),
    #[error("unknown database: {0}")]
    UnknownDatabase(String),
    #[error("unsupported connection scheme '{0}' (only mysql is supported today)")]
    UnsupportedScheme(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Connection parameters for a single named connection (as read from config).
#[derive(Debug, Clone)]
pub struct ConnConfig {
    pub name: String,
    pub url: String,
    pub read_only: bool,
}

/// A single cell value, normalized across backends.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    /// Backend-specific value that doesn't map cleanly onto the variants
    /// above; carries its textual representation.
    Other(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Null => write!(f, "NULL"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Text(s) => write!(f, "{s}"),
            Value::Bytes(b) => write!(f, "0x{}", hex_encode(b)),
            Value::Other(s) => write!(f, "{s}"),
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A single result row: column names travel with every row so streaming
/// consumers never need a separate metadata round-trip.
#[derive(Debug, Clone)]
pub struct Row {
    pub cols: Vec<String>,
    pub values: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub ty: String,
    pub nullable: bool,
    pub key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ForeignKey {
    pub column: String,
    pub ref_table: String,
    pub ref_column: String,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    pub indexes: Vec<String>,
    pub foreign_keys: Vec<ForeignKey>,
}

/// Database names visible on the server — cheap to load (no table walk),
/// used to populate the sidebar tree immediately on connect/expand.
/// Each database's actual tables are fetched lazily via
/// [`Driver::tables`], only once the user opens that database.
#[derive(Debug, Clone)]
pub struct Schema {
    pub databases: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub rows: Vec<Row>,
}

/// SQL-dialect-specific string building. Kept minimal on purpose: only what
/// the TUI/CLI actually needs today (identifier quoting, previewing a
/// `LIMIT`, and a reserved-word vocabulary for the editor's syntax
/// highlighting/autocomplete — each engine has its own).
pub trait Dialect: Send + Sync {
    fn quote_ident(&self, s: &str) -> String;
    fn limit(&self, sql: &str, n: u64) -> String;
    /// Reserved words this dialect recognizes, uppercase. Used by the TUI
    /// to classify tokens for syntax highlighting and to offer as
    /// autocomplete candidates — never sent to the server, purely a
    /// presentation-layer vocabulary.
    fn keywords(&self) -> &'static [&'static str];
}

/// Backend contract. The TUI only ever talks to this trait; it never knows
/// which engine is underneath.
#[async_trait]
pub trait Driver: Send + Sync {
    async fn connect(cfg: &ConnConfig) -> Result<Self, DriverError>
    where
        Self: Sized;

    /// Streams rows so callers never have to buffer a full result set;
    /// `cancel` lets the UI abort a running query (e.g. on `Ctrl+C`).
    fn query<'a>(
        &'a self,
        sql: &'a str,
        cancel: CancellationToken,
    ) -> BoxStream<'a, anyhow::Result<Row>>;

    /// Runs a statement that doesn't return rows; returns affected-row count.
    async fn execute(&self, sql: &str) -> anyhow::Result<u64>;

    async fn schema(&self) -> anyhow::Result<Schema>;

    /// Loads every table in `db_name` (columns + indexes + foreign keys).
    /// Deliberately separate from `schema()`, which only lists database
    /// names — a server with many databases/tables would otherwise pay
    /// for walking `information_schema` for every table in every database
    /// just to expand one connection in the sidebar.
    async fn tables(&self, db_name: &str) -> anyhow::Result<Vec<Table>>;

    /// Lists databases/schemas visible on the server. Identical in cost to
    /// `schema()` (both are just a database-name listing); kept as its own
    /// method because the "add connection" wizard wants a plain
    /// `Vec<String>` before any connection is otherwise established in
    /// `App` state, whereas the sidebar wants it wrapped in [`Schema`].
    async fn list_databases(&self) -> anyhow::Result<Vec<String>>;

    async fn explain(&self, sql: &str) -> anyhow::Result<Plan>;

    fn dialect(&self) -> &dyn Dialect;
}
