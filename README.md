# sqldr

TUI para administrar bases de datos (MySQL hoy; Postgres y SQLite planeados)
al estilo [herdr](https://github.com/herdrdev/herdr): un binario, sidebar con
estado de conexiones, editor + resultados, pensado para uso diario desde la
terminal.

`sqldr-core` no sabe nada de terminal ni UI — solo expone el trait `Driver`
y los tipos que cualquier motor implementa. La TUI (`sqldr`) habla
exclusivamente con ese trait, nunca con MySQL directamente.

## Estado actual

- **Motor soportado:** MySQL (vía [`sqlx`](https://github.com/launchbadge/sqlx)).
- **CLI:** `sqldr query`, `sqldr conn set-password`.
- **TUI:** sidebar (conexiones → bases de datos → tabs de tabla), editor SQL,
  resultados paginados, historial, guardarraíles, mouse, wizard de alta de
  conexiones.

## Requisitos

- Rust estable (`rustup update stable` o el toolchain de tu distro), edición 2021.
- Un servidor MySQL accesible (local o remoto) para conectar.
- Backend de keyring del sistema (D-Bus secret-service en Linux, Keychain en
  macOS, Credential Manager en Windows) para guardar contraseñas.

## Compilar

```bash
git clone git@github.com:dparmijog/sqldr.git
cd sqldr
cargo build --workspace --release
```

El binario queda en `target/release/sqldr` (o `target/debug/sqldr` con
`cargo build` sin `--release`).

```bash
cargo test --workspace   # corre los tests unitarios (guard, pagination, clipboard, app)
```

## Configuración

`sqldr` lee `~/.config/sqldr/config.toml` (según XDG/directories del SO):

```toml
[[connections]]
name = "kennis-dev"
url = "mysql://root@localhost:3306/kennis"
read_only = false

[[connections]]
name = "kennis-prod"
url = "mysql://user@prod-host:3306/kennis"
read_only = true          # bloquea DML/DDL, barra roja en la UI
```

Las contraseñas **no** se guardan en el archivo — van al keyring del
sistema, asociadas al nombre de la conexión:

```bash
sqldr conn set-password kennis-dev
```

También podés crear conexiones sin tocar el archivo: dentro de la TUI,
`Ctrl+N` abre un wizard que elige el motor, prueba las credenciales contra
el servidor real, y te deja elegir la base de datos de una lista antes de
guardar.

## Uso

### CLI

```bash
sqldr query -c kennis-dev "SELECT * FROM users LIMIT 10"
```

Imprime las filas como tabla. Si la conexión es `read_only`, cualquier
sentencia de escritura se rechaza antes de tocar la red. Un `UPDATE`/`DELETE`
sin `WHERE` pide confirmación interactiva por stdin.

### TUI

```bash
sqldr
```

Sin subcomando, arranca la interfaz interactiva.

| Tecla | Acción |
|---|---|
| `Tab` / `Shift+Tab` | ciclar foco sidebar → editor → resultados |
| `↑`/`↓`/`Enter` en sidebar | navegar conexiones/bases; seleccionar una base abre una tab con sus tablas |
| `←`/`→` en una tab | cambiar entre tabs de bases de datos abiertas |
| `x` en una tab | cerrar la tab activa |
| `Esc` en una tab | volver al árbol de conexiones |
| `/` en sidebar | buscar tablas por nombre en todas las conexiones |
| `Ctrl+Enter` / `F5` | ejecutar la query del editor (`Ctrl+Enter` requiere terminal con protocolo Kitty; `F5` funciona en cualquiera) |
| `Ctrl+C` | cancelar la query en curso |
| `Ctrl+R` | historial de queries de la conexión activa |
| `Ctrl+E` | editar la query en `$EDITOR` |
| `Ctrl+N` | wizard para agregar una conexión nueva |
| `PageUp`/`PageDown` en resultados | paginar (toda `SELECT` sin `LIMIT` propio recibe uno automático de 500 filas) |
| `y` / `Y` / `c` / `i` en resultados | copiar celda / fila como JSON / CSV / `INSERT` (vía OSC 52, funciona sobre SSH) |
| Mouse | click para enfocar/seleccionar, arrastrar los bordes para redimensionar sidebar/editor |
| `q` | salir |

Todo lo que se ejecuta se refleja en el editor (incluido el `LIMIT`/`OFFSET`
automático), así que podés editarlo y volver a correrlo — si borrás el
límite que agregó `sqldr`, se respeta y corre sin límite.

## Arquitectura

```
crates/
  sqldr-core/     # trait Driver, tipos (Row, Value, Schema...), sin UI
    driver.rs
    mysql.rs      # implementación con sqlx
    guard.rs      # guardarraíles: is_mutating, needs_where_confirmation
    history.rs    # historial de queries por conexión (JSONL)
    pagination.rs # LIMIT/OFFSET automático
  sqldr/          # TUI + CLI
    main.rs       # clap: `sqldr` (TUI) y `sqldr query`/`conn`
    app.rs        # estado global + event loop
    config.rs     # config.toml + keyring
    clipboard.rs  # OSC 52 + formatos JSON/CSV/INSERT
    tui.rs        # terminal setup, integración $EDITOR
    ui/           # sidebar, editor, resultados, overlays, statusbar
docs/archive/     # ADRs: decisiones no triviales documentadas
```

## Contribuir

- `cargo fmt` y `cargo clippy -- -D warnings` antes de cada cambio (si tu
  entorno no tiene esos componentes instalados, al menos `cargo build
  --workspace` y `cargo test --workspace` deben pasar limpio).
- Decisiones no triviales van a `docs/archive/` como ADR.
- Commits pequeños, uno por cambio lógico.

## Licencia

Por definir.
