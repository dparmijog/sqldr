//! MySQL backend: implements [`Driver`] on top of `sqlx::MySqlPool`.

use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};
use sqlx::mysql::{MySqlPool, MySqlRow};
use sqlx::{Column as _, Row as _, TypeInfo as _};
use tokio_util::sync::CancellationToken;

use crate::driver::{Column, ConnConfig, Dialect, Driver, ForeignKey, Plan, Row, Schema, Table, Value};

pub struct MySqlDialect;

impl Dialect for MySqlDialect {
    fn quote_ident(&self, s: &str) -> String {
        format!("`{}`", s.replace('`', "``"))
    }

    fn limit(&self, sql: &str, n: u64) -> String {
        let trimmed = sql.trim_end().trim_end_matches(';');
        format!("{trimmed} LIMIT {n}")
    }
}

pub struct MySqlDriver {
    pool: MySqlPool,
    dialect: MySqlDialect,
}

/// Decodes a single MySQL cell into our backend-agnostic [`Value`], based on
/// the column's reported type name. Unknown types fall back to their textual
/// form rather than failing the whole row.
fn decode_value(row: &MySqlRow, idx: usize, type_name: &str) -> anyhow::Result<Value> {
    macro_rules! try_as {
        ($t:ty, $wrap:expr) => {
            row.try_get::<Option<$t>, _>(idx)
                .map(|v| v.map($wrap).unwrap_or(Value::Null))
        };
    }

    let value = match type_name {
        "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT" | "YEAR" => {
            try_as!(i64, Value::Int)
        }
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED" => {
            try_as!(i64, Value::Int)
        }
        "BIGINT UNSIGNED" => row
            .try_get::<Option<u64>, _>(idx)
            .map(|v| v.map(|n| Value::Text(n.to_string())).unwrap_or(Value::Null)),
        "FLOAT" | "DOUBLE" => try_as!(f64, Value::Float),
        "DECIMAL" | "NUMERIC" => row
            .try_get::<Option<rust_decimal::Decimal>, _>(idx)
            .map(|v| v.map(|d| Value::Text(d.to_string())).unwrap_or(Value::Null)),
        "VARCHAR" | "CHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" | "ENUM" | "SET"
        | "JSON" => try_as!(String, Value::Text),
        "DATE" => row
            .try_get::<Option<chrono::NaiveDate>, _>(idx)
            .map(|v| v.map(|d| Value::Text(d.to_string())).unwrap_or(Value::Null)),
        "TIME" => row
            .try_get::<Option<chrono::NaiveTime>, _>(idx)
            .map(|v| v.map(|t| Value::Text(t.to_string())).unwrap_or(Value::Null)),
        "DATETIME" => row
            .try_get::<Option<chrono::NaiveDateTime>, _>(idx)
            .map(|v| v.map(|t| Value::Text(t.to_string())).unwrap_or(Value::Null)),
        "TIMESTAMP" => row
            .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(idx)
            .map(|v| v.map(|t| Value::Text(t.to_string())).unwrap_or(Value::Null)),
        "BOOLEAN" => try_as!(bool, Value::Bool),
        "TINYBLOB" | "BLOB" | "MEDIUMBLOB" | "LONGBLOB" | "VARBINARY" | "BINARY" => {
            // MySQL reports VARCHAR/TEXT columns with a binary collation
            // (common in information_schema and `_bin` charsets) using
            // these same blob type names; prefer text when it's valid
            // UTF-8, falling back to raw bytes otherwise.
            match row.try_get::<Option<Vec<u8>>, _>(idx) {
                Ok(Some(bytes)) => Ok(String::from_utf8(bytes)
                    .map(Value::Text)
                    .unwrap_or_else(|e| Value::Bytes(e.into_bytes()))),
                Ok(None) => Ok(Value::Null),
                Err(e) => Err(e),
            }
        }
        "BIT" => row
            .try_get::<Option<u64>, _>(idx)
            .map(|v| v.map(|n| Value::Text(n.to_string())).unwrap_or(Value::Null)),
        other => {
            // Best-effort fallback: try a string decode; if that fails too,
            // surface the type name so at least the shape is visible.
            match row.try_get::<Option<String>, _>(idx) {
                Ok(v) => Ok(v.map(Value::Text).unwrap_or(Value::Null)),
                Err(_) => Ok(Value::Other(format!("<{other}>"))),
            }
        }
    };

    value.map_err(|e| anyhow::anyhow!("decoding column {idx} ({type_name}): {e}"))
}

fn row_from_mysql(row: MySqlRow) -> anyhow::Result<Row> {
    let mut cols = Vec::with_capacity(row.columns().len());
    let mut values = Vec::with_capacity(row.columns().len());
    for (idx, col) in row.columns().iter().enumerate() {
        cols.push(col.name().to_string());
        values.push(decode_value(&row, idx, col.type_info().name())?);
    }
    Ok(Row { cols, values })
}

/// Decodes column `idx` as text, tolerating MySQL's binary-collation
/// string columns (`sqlx`'s `Decode<MySql, String>` refuses any column
/// with the `BINARY` flag set, which `information_schema` columns carry
/// on this server despite holding plain readable text). Required because
/// `schema()` reads catalog columns with `sqlx::query`/manual mapping
/// instead of `query_as`, precisely to work around that restriction.
fn opt_text_col(row: &MySqlRow, idx: usize) -> anyhow::Result<Option<String>> {
    match row.try_get::<Option<String>, _>(idx) {
        Ok(v) => Ok(v),
        Err(_) => match row.try_get::<Option<Vec<u8>>, _>(idx) {
            Ok(Some(bytes)) => Ok(Some(
                String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("{e}"))?,
            )),
            Ok(None) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("decoding column {idx} as text: {e}")),
        },
    }
}

fn text_col(row: &MySqlRow, idx: usize) -> anyhow::Result<String> {
    opt_text_col(row, idx)?.ok_or_else(|| anyhow::anyhow!("column {idx} is NULL, expected text"))
}

#[async_trait]
impl Driver for MySqlDriver {
    async fn connect(cfg: &ConnConfig) -> anyhow::Result<Self> {
        let pool = MySqlPool::connect(&cfg.url).await?;
        Ok(MySqlDriver { pool, dialect: MySqlDialect })
    }

    fn query<'a>(
        &'a self,
        sql: &'a str,
        cancel: CancellationToken,
    ) -> BoxStream<'a, anyhow::Result<Row>> {
        let rows = sqlx::query(sql)
            .fetch(&self.pool)
            .map(|res| res.map_err(anyhow::Error::from).and_then(row_from_mysql));
        rows.take_until(cancel.cancelled_owned()).boxed()
    }

    async fn execute(&self, sql: &str) -> anyhow::Result<u64> {
        let result = sqlx::query(sql).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    async fn schema(&self) -> anyhow::Result<Schema> {
        Ok(Schema { databases: list_database_names(&self.pool).await? })
    }

    async fn tables(&self, db_name: &str) -> anyhow::Result<Vec<Table>> {
        let table_rows = sqlx::query(
            "SELECT TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME",
        )
        .bind(db_name)
        .fetch_all(&self.pool)
        .await?;
        let table_names = table_rows
            .iter()
            .map(|r| text_col(r, 0))
            .collect::<anyhow::Result<Vec<_>>>()?;

        // One query for every table's columns, one for every table's
        // indexes, and one for every table's foreign keys — instead of
        // per-table round trips — so opening a database with hundreds of
        // tables costs four queries total, not `3 * tables + 1`.
        let col_rows = sqlx::query(
            "SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, \
                    NULLIF(COLUMN_KEY, '') \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME, ORDINAL_POSITION",
        )
        .bind(db_name)
        .fetch_all(&self.pool)
        .await?;
        let mut columns_by_table: std::collections::HashMap<String, Vec<Column>> =
            std::collections::HashMap::new();
        for r in &col_rows {
            let table_name = text_col(r, 0)?;
            let column = Column {
                name: text_col(r, 1)?,
                ty: text_col(r, 2)?,
                nullable: text_col(r, 3)?.eq_ignore_ascii_case("YES"),
                key: opt_text_col(r, 4)?,
            };
            columns_by_table.entry(table_name).or_default().push(column);
        }

        let idx_rows = sqlx::query(
            "SELECT DISTINCT TABLE_NAME, INDEX_NAME FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = ? ORDER BY TABLE_NAME, INDEX_NAME",
        )
        .bind(db_name)
        .fetch_all(&self.pool)
        .await?;
        let mut indexes_by_table: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for r in &idx_rows {
            let table_name = text_col(r, 0)?;
            indexes_by_table.entry(table_name).or_default().push(text_col(r, 1)?);
        }

        let fk_rows = sqlx::query(
            "SELECT TABLE_NAME, COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
             FROM information_schema.KEY_COLUMN_USAGE \
             WHERE TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY TABLE_NAME, COLUMN_NAME",
        )
        .bind(db_name)
        .fetch_all(&self.pool)
        .await?;
        let mut fks_by_table: std::collections::HashMap<String, Vec<ForeignKey>> =
            std::collections::HashMap::new();
        for r in &fk_rows {
            let table_name = text_col(r, 0)?;
            let fk = ForeignKey {
                column: text_col(r, 1)?,
                ref_table: text_col(r, 2)?,
                ref_column: text_col(r, 3)?,
            };
            fks_by_table.entry(table_name).or_default().push(fk);
        }

        let tables = table_names
            .into_iter()
            .map(|name| {
                let columns = columns_by_table.remove(&name).unwrap_or_default();
                let indexes = indexes_by_table.remove(&name).unwrap_or_default();
                let foreign_keys = fks_by_table.remove(&name).unwrap_or_default();
                Table { name, columns, indexes, foreign_keys }
            })
            .collect();

        Ok(tables)
    }

    async fn list_databases(&self) -> anyhow::Result<Vec<String>> {
        list_database_names(&self.pool).await
    }

    async fn explain(&self, sql: &str) -> anyhow::Result<Plan> {
        let explain_sql = format!("EXPLAIN {sql}");
        let raw_rows = sqlx::query(&explain_sql).fetch_all(&self.pool).await?;
        let rows = raw_rows
            .into_iter()
            .map(row_from_mysql)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Plan { rows })
    }

    fn dialect(&self) -> &dyn Dialect {
        &self.dialect
    }
}

/// Lists user databases, filtering out MySQL's own system schemas.
async fn list_database_names(pool: &MySqlPool) -> anyhow::Result<Vec<String>> {
    let db_rows = sqlx::query(
        "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA \
         WHERE SCHEMA_NAME NOT IN ('mysql','information_schema','performance_schema','sys') \
         ORDER BY SCHEMA_NAME",
    )
    .fetch_all(pool)
    .await?;
    db_rows.iter().map(|r| text_col(r, 0)).collect()
}
