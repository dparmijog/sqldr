# sqldr

A database administration TUI (MySQL today; Postgres and SQLite planned)
in the style of [herdr](https://github.com/herdrdev/herdr): a single
binary, a sidebar with connection status, an editor + results pane, built
for daily use from the terminal.

`sqldr-core` knows nothing about terminals or UI — it only exposes the
`Driver` trait and the types any engine implements. The TUI (`sqldr`)
speaks exclusively to that trait, never to MySQL directly.

## Current status

- **Supported engine:** MySQL (via [`sqlx`](https://github.com/launchbadge/sqlx)).
- **CLI:** `sqldr query`, `sqldr conn set-password`.
- **TUI:** sidebar (connections → databases → per-table tabs), SQL editor,
  paginated results, history, guardrails, mouse support, a connection
  wizard that can also edit/delete existing connections, a background
  heartbeat that flags a dropped connection on its own, `EXPLAIN`,
  and table-structure/foreign-key navigation.

## Requirements

- Stable Rust (`rustup update stable` or your distro's toolchain), edition 2021.
- A reachable MySQL server (local or remote) to connect to.
- A system keyring backend (D-Bus secret-service on Linux, Keychain on
  macOS, Credential Manager on Windows) to store passwords.

## Building

```bash
git clone git@github.com:dparmijog/sqldr.git
cd sqldr
cargo build --workspace --release
```

The binary ends up at `target/release/sqldr` (or `target/debug/sqldr` with
a plain `cargo build`).

```bash
cargo test --workspace   # runs unit tests (guard, pagination, clipboard, app)
```

## Configuration

`sqldr` reads `~/.config/sqldr/config.toml` (per your OS's XDG/directories
convention):

```toml
[[connections]]
name = "kennis-dev"
url = "mysql://root@localhost:3306/kennis"
read_only = false

[[connections]]
name = "kennis-prod"
url = "mysql://user@prod-host:3306/kennis"
read_only = true          # blocks DML/DDL, red bar in the UI
```

Passwords are **not** stored in the file — they go to the system keyring,
keyed by connection name:

```bash
sqldr conn set-password kennis-dev
```

You can also create connections without touching the file: inside the
TUI, `Ctrl+N` opens a wizard that picks the engine, tests credentials
against the real server, and lets you choose the database from a live
list before saving.

## Usage

### CLI

```bash
sqldr query -c kennis-dev "SELECT * FROM users LIMIT 10"
```

Prints rows as a table. On a `read_only` connection, any write statement
is rejected before it ever reaches the network. An `UPDATE`/`DELETE`
without a `WHERE` clause prompts for interactive confirmation on stdin.

### TUI

```bash
sqldr
```

With no subcommand, launches the interactive interface.

| Key | Action |
|---|---|
| `Tab` / `Shift+Tab` | cycle focus: sidebar → editor → results |
| `↑`/`↓`/`Enter` in sidebar | navigate connections/databases; selecting a database opens a tab with its tables |
| `e` / `d` on a connection | edit / delete that connection (with a confirmation before deleting) |
| `s` on a table row | read-only structure view: columns, indexes, foreign keys |
| `←`/`→` in a tab | switch between open database tabs |
| `x` in a tab | close the active tab |
| `Esc` in a tab | go back to the connection tree |
| `/` in sidebar | search tables by name across every connection |
| `Ctrl+Enter` / `F5` | run the editor's query (`Ctrl+Enter` needs a Kitty-protocol terminal; `F5` works everywhere) |
| `Ctrl+C` | cancel the running query |
| `Ctrl+R` | query history for the active connection |
| `Ctrl+E` | edit the query in `$EDITOR` |
| `Ctrl+N` | wizard to add a new connection |
| `Ctrl+X` | run the editor's query through `EXPLAIN` instead of executing it |
| `PageUp`/`PageDown` in results | page through results (every `SELECT` without its own `LIMIT` gets an automatic 500-row one) |
| `g` in results | follow a foreign-key cell to the referenced row (needs a table preview, not an arbitrary query) |
| `y` / `Y` / `c` / `i` in results | copy cell / row as JSON / CSV / `INSERT` (via OSC 52, works over SSH) |
| Mouse | click to focus/select, drag borders to resize sidebar/editor |
| `q` | quit |

Whatever actually runs is mirrored into the editor (including the
auto-applied `LIMIT`/`OFFSET`), so you can edit and rerun it — if you
delete the limit `sqldr` added, that's honored and it runs unbounded.

## Architecture

```
crates/
  sqldr-core/     # Driver trait, types (Row, Value, Schema...), no UI
    driver.rs
    mysql.rs      # sqlx-backed implementation
    guard.rs      # guardrails: is_mutating, needs_where_confirmation
    history.rs    # per-connection query history (JSONL)
    pagination.rs # automatic LIMIT/OFFSET
  sqldr/          # TUI + CLI
    main.rs       # clap: `sqldr` (TUI) and `sqldr query`/`conn`
    app.rs        # global state + event loop
    config.rs     # config.toml + keyring
    clipboard.rs  # OSC 52 + JSON/CSV/INSERT formats
    tui.rs        # terminal setup, $EDITOR integration
    ui/           # sidebar, editor, results, overlays, statusbar
docs/archive/     # ADRs: non-trivial decisions, documented
```

## Contributing

- Run `cargo fmt` and `cargo clippy -- -D warnings` before each change (if
  your environment doesn't have those components installed, at minimum
  `cargo build --workspace` and `cargo test --workspace` must pass clean).
- Non-trivial decisions go to `docs/archive/` as an ADR.
- Small commits, one per logical change.

## License

TBD.
