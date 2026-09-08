# sqldr — guía de arranque

TUI para administrar bases de datos (MySQL primero; Postgres y SQLite después), con la mentalidad de herdr: un binario, panes/tabs, sidebar con estado, y a futuro daemon + socket API para agentes.

Foco de la primera versión: **uso personal desde la terminal**. El socket API viene después.

## 1. Requisitos

- Rust estable (`rustup update stable`), edición 2021.
- MySQL 8 local para pruebas (`docker run -d --name sqldr-mysql -e MYSQL_ROOT_PASSWORD=root -p 3306:3306 mysql:8`).
- `cargo install cargo-watch` (opcional, para `cargo watch -x run`).

## 2. Crear el workspace

```bash
mkdir sqldr && cd sqldr && git init
cargo new --lib crates/sqldr-core
cargo new --bin crates/sqldr
```

`Cargo.toml` raíz:

```toml
[workspace]
resolver = "2"
members = ["crates/sqldr-core", "crates/sqldr"]

[workspace.dependencies]
tokio = { version = "1", features = ["full"] }
tokio-util = "0.7"
anyhow = "1"
thiserror = "1"
serde = { version = "1", features = ["derive"] }
futures = "0.3"
```

### `crates/sqldr-core/Cargo.toml`

```toml
[dependencies]
tokio.workspace = true
tokio-util.workspace = true
anyhow.workspace = true
thiserror.workspace = true
serde.workspace = true
futures.workspace = true
async-trait = "0.1"
sqlx = { version = "0.8", features = ["runtime-tokio", "mysql", "chrono", "rust_decimal", "json"] }
```

### `crates/sqldr/Cargo.toml`

```toml
[dependencies]
sqldr-core = { path = "../sqldr-core" }
tokio.workspace = true
anyhow.workspace = true
serde.workspace = true
ratatui = "0.29"
crossterm = { version = "0.28", features = ["event-stream"] }
tui-textarea = "0.7"
toml = "0.8"
directories = "5"
keyring = "3"
clap = { version = "4", features = ["derive"] }
```

## 3. Estructura

```
sqldr/
  Cargo.toml
  crates/
    sqldr-core/src/
      lib.rs
      driver.rs       # trait Driver + tipos (Row, Value, Schema, Plan)
      mysql.rs        # impl Driver con sqlx
      schema.rs       # cache de esquema
      guard.rs        # detección de UPDATE/DELETE sin WHERE, etc.
      history.rs      # historial de queries por conexión
    sqldr/src/
      main.rs         # clap: `sqldr` (TUI) y `sqldr query -c <conn> "<sql>"`
      config.rs       # ~/.config/sqldr/config.toml
      app.rs          # estado global + loop de eventos
      ui/
        mod.rs
        layout.rs     # sidebar | editor / resultados
        sidebar.rs    # árbol conexiones → bases → tablas
        editor.rs     # tui-textarea, modo vim
        results.rs    # tabla con scroll virtual, vista vertical de fila
        statusbar.rs  # conexión activa, estado, modo read-only
      keymap.rs
  docs/
    archive/          # ADRs y decisiones (mismo esquema que en los otros repos)
```

Regla: `sqldr-core` no sabe nada de terminal ni UI. La TUI no sabe qué motor hay debajo; solo habla con el trait.

## 4. El trait (definirlo antes que todo)

```rust
// sqldr-core/src/driver.rs
use async_trait::async_trait;
use futures::stream::BoxStream;
use tokio_util::sync::CancellationToken;

pub struct ConnConfig { pub name: String, pub url: String, pub read_only: bool }

pub enum Value { Null, Bool(bool), Int(i64), Float(f64), Text(String), Bytes(Vec<u8>), Other(String) }
pub struct Row { pub cols: Vec<String>, pub values: Vec<Value> }

pub struct Column { pub name: String, pub ty: String, pub nullable: bool, pub key: Option<String> }
pub struct Table  { pub name: String, pub columns: Vec<Column>, pub indexes: Vec<String> }
pub struct Schema { pub databases: Vec<(String, Vec<Table>)> }

pub struct Plan { pub rows: Vec<Row> }

pub trait Dialect {
    fn quote_ident(&self, s: &str) -> String;
    fn limit(&self, sql: &str, n: u64) -> String;
}

#[async_trait]
pub trait Driver: Send + Sync {
    async fn connect(cfg: &ConnConfig) -> anyhow::Result<Self> where Self: Sized;
    fn query<'a>(&'a self, sql: &'a str, cancel: CancellationToken) -> BoxStream<'a, anyhow::Result<Row>>;
    async fn execute(&self, sql: &str) -> anyhow::Result<u64>;     // filas afectadas
    async fn schema(&self) -> anyhow::Result<Schema>;
    async fn explain(&self, sql: &str) -> anyhow::Result<Plan>;
    fn dialect(&self) -> &dyn Dialect;
}
```

`query` devuelve un stream para nunca cargar el resultado completo en memoria; la UI pide filas por lote y el `CancellationToken` permite abortar con `Ctrl+C`.

## 5. Config

`~/.config/sqldr/config.toml`:

```toml
[[connections]]
name = "kennis-dev"
url = "mysql://root@localhost:3306/kennis"
read_only = false

[[connections]]
name = "kennis-prod"
url = "mysql://user@prod-host:3306/kennis"
read_only = true          # barra roja, bloquea DML/DDL
```

Passwords en `keyring` (servicio `sqldr`, usuario = nombre de conexión). `sqldr conn set-password <name>` las guarda.

## 6. Hitos

1. **core + CLI** — `sqldr query -c kennis-dev "select 1"` imprime filas. Sirve para probar el driver sin UI.
2. **TUI fija** — layout sidebar | editor / resultados. Ejecutar con `Ctrl+Enter`, cancelar con `Ctrl+C`, `Enter` en tabla = `SELECT * … LIMIT 200`.
3. **Ergonomía** — historial (`Ctrl+R`), abrir query en `$EDITOR` (`Ctrl+E`), guardarraíles (DML sin `WHERE` pide confirmación), modo read-only, copiar celda/fila como JSON/CSV/INSERT.
4. **Estado en sidebar** — `idle / running / error / locked` por conexión. Para MySQL: `SHOW PROCESSLIST` + `information_schema.INNODB_TRX` para ver bloqueos.
5. **Panes y tabs** — árbol de layout con splits libres (mirar cómo lo modela herdr).
6. **Daemon + socket API** — conexiones persistentes, detach/attach, `sqldr socket query --json` para agentes.
7. **Postgres, luego SQLite** — solo implementar el trait.

## 7. Keymap inicial (modo vim en editor)

| Tecla | Acción |
|---|---|
| `Tab` / `Shift+Tab` | ciclar foco sidebar → editor → resultados |
| `Ctrl+Enter` | ejecutar query (o selección) |
| `Ctrl+C` | cancelar query en curso |
| `Ctrl+E` | editar query en `$EDITOR` |
| `Ctrl+R` | historial |
| `/` en sidebar | búsqueda fuzzy de tablas |
| `Enter` en tabla | preview `LIMIT 200` |
| `v` en resultados | fila en vista vertical |
| `y` en resultados | copiar celda; `Y` fila como JSON |
| `q` | salir |

## 8. Convenciones

- Toda decisión no trivial (ej. por qué stream y no `fetch_all`, por qué `keyring`) va a `docs/archive/` como ADR.
- Commits pequeños; un hito = una rama.
- `cargo clippy -- -D warnings` y `cargo fmt` antes de cada merge.
- Primer commit: workspace + trait + `mysql.rs` con `connect` y `query` funcionando contra el contenedor local.

## 9. Referencias para robar ideas

- herdr (`github.com/herdrdev/herdr`) — árbol de panes, daemon, socket.
- lazysql, rainfrog, gobang — TUIs de SQL en Go/Rust; ver qué se siente bien y qué no.
- ratatui book (`ratatui.rs`) — patrones de app state + event loop con tokio.
