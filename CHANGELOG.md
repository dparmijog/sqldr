# Changelog

## [1.2.0] - 2026-09-09

### Añadido
- **`EXPLAIN` accesible desde la TUI** (`Ctrl+X` / `F6`): corre la query
  del editor a través de `EXPLAIN` y muestra el plan en el panel de
  resultados, sin pasar por el guardrail de mutación ni la confirmación
  de `WHERE` (`EXPLAIN` nunca toca datos). El backend ya lo tenía
  implementado desde la 1.0; simplemente nunca estaba cableado a ninguna
  tecla.
- **Editar y borrar conexiones desde la TUI** (`e` / `d` sobre un nodo de
  conexión): antes solo se podía agregar una conexión nueva; ahora el
  wizard se reabre pre-llenado para editar (con fallback al password ya
  guardado en el keyring para el test en vivo si el campo queda en
  blanco), y borrar pide confirmación antes de limpiar `config.toml`, el
  keyring, y cualquier tab abierto de esa conexión.
- **Heartbeat de conexión**: un `SELECT 1` de fondo cada 20s detecta sola
  una conexión caída (antes solo se notaba al ejecutar la próxima
  query), y el sidebar muestra `(Ns)` desde el último ping exitoso.
- **Explorador de estructura de tabla** (`s` sobre una fila de tabla):
  vista de solo lectura con columnas (tipo/nulabilidad/key), índices y
  foreign keys — sin query adicional, ya estaba cargado.
- **Navegación por foreign key** (`g` sobre una celda de resultados): si
  la columna bajo el cursor es un FK, salta a la fila referenciada
  (`SELECT * FROM tabla_ref WHERE col_ref = valor`). Requiere que la
  tabla se haya abierto como preview desde el sidebar (no funciona sobre
  una query arbitraria tipeada a mano).
- `sqldr-core`: `Driver::tables()` ahora también trae metadata de
  foreign keys (`information_schema.KEY_COLUMN_USAGE`), en batch junto a
  columnas e índices (4 queries por base en vez de 3).
- `docs/IDEAS.md`: backlog de ideas priorizado para futuras iteraciones.

### Corregido
- **Bug real encontrado en vivo**: editar una conexión dejando el campo
  de password en blanco (para no cambiarlo) hacía fallar el test de
  credenciales con `1045 Access denied`, porque el test en vivo no caía
  de vuelta al password ya guardado en el keyring. Corregido.
- **Bug real encontrado en vivo**: borrar una conexión mientras otra
  conexión (con heartbeat activo) quedaba en un índice posterior dejaba
  el heartbeat de esa otra conexión apuntando a un índice viejo/inválido
  tras el corrimiento de la lista — sus eventos de heartbeat se perdían
  silenciosamente o corrompían el estado de otra conexión. Corregido
  re-lanzando el heartbeat de cada conexión sobreviviente con su índice
  correcto tras un borrado.
- **`EXPLAIN` no daba ningún feedback con el editor vacío**: reportado
  en vivo por el usuario ("no pasa nada" al presionar `Ctrl+X`/`F6`
  navegando por el sidebar sin haber escrito una query) — el chequeo de
  "editor vacío" retornaba en silencio antes de mostrar cualquier error.
  Ahora muestra `nothing to explain: the editor is empty`.
- Se agregó `F6` como fallback sin modificador para `EXPLAIN` (mismo
  patrón que `Ctrl+Enter`/`F5` ya usa para ejecutar), para terminales/
  multiplexores que no reenvíen `Ctrl+<letra>` de forma confiable.

---

## [1.0.0]

Primera versión estable: TUI + CLI funcional para administrar bases MySQL,
con editor SQL, navegación de esquema, historial de queries, wizard de
conexión y guardrails de seguridad para DML sin `WHERE`.

### Añadido
- Workspace Rust (`sqldr-core` + `sqldr`), trait `Driver` y backend MySQL
  sobre `sqlx`, con decodificación de tipos (enteros con/sin signo, floats,
  `DECIMAL`, texto, `DATE`/`TIME`/`DATETIME`/`TIMESTAMP`, `BOOLEAN`,
  binarios) y consultas cancelables.
- CLI mínima: `sqldr query -c <conn> "<sql>"` imprime resultados en tabla ASCII.
- Fix de decodificación: `BIT` como `u64` (no bytes crudos) y reconocimiento
  correcto de tipos de la familia `TEXT`.
- TUI completa con layout de tres paneles (sidebar / editor / resultados),
  event loop, navegación del árbol de conexiones/esquema y guardrails.
- Historial de queries por conexión (`Ctrl+R`), edición en `$EDITOR` externo
  (`Ctrl+E`), confirmación obligatoria para DML sin `WHERE`, y copiar
  celda/fila (JSON, CSV, `INSERT`) vía OSC 52.
- `$EDITOR` tokenizado shell-style (soporta `"code --wait"`, `"vim -u NONE"`, etc.).
- El historial de queries por CLI solo se registra si la conexión tuvo éxito,
  igual que en la TUI.
- Soporte de mouse: click para enfocar/seleccionar paneles, arrastrar para
  redimensionar sidebar y editor.
- Wizard de conexión (`Ctrl+N`): selección de motor → credenciales → test
  en vivo contra el servidor → selección de base de datos real (en vez de
  tipear el nombre a ciegas).
- Fix: un resultado de test de conexión obsoleto (`WizardTested`) ya no
  pisaba overlays no relacionados; se agregaron tests de regresión.
- Fix de scroll del sidebar (se mueve relativo al cursor) y búsqueda de
  tablas con `/`.
- Tabs por base de datos seleccionada en el sidebar; paginación automática
  (`LIMIT 500` + `PgUp`/`PgDn`).
- Fix de detección de `LIMIT` en paginación: ahora es consciente de la
  profundidad de paréntesis (ignora `LIMIT` dentro de subqueries).
- El editor se sincroniza para mostrar el SQL exacto que se ejecutó
  (incluyendo el `LIMIT`/`OFFSET` automático), y se actualiza con la
  paginación.
- Si se borra el `LIMIT` agregado automáticamente y se vuelve a ejecutar,
  la query corre sin límite (se respeta la intención explícita del usuario).
- README con instrucciones de build, configuración, uso y mapa de teclas.

### Quitado
- `START_HERE.md` (documento de scaffolding inicial), reemplazado por `README.md`.

### Versión
- Bump de ambos crates a `1.0.0`, se expone la flag `--version`.

---

## [1.1.0] - 2026-09-09

### Añadido
- **Sistema de temas con diálogo de opciones** (`Ctrl+O`): selector de temas
  (Dark, Dracula, Nord, Catppuccin Mocha, etc.) con preview en vivo mientras
  se navega, y persistencia en `config.toml`. De paso, se modularizó
  `app.rs` en el submódulo `app/`.
- **Grid de resultados real**: la tabla de resultados renderiza con columnas
  auto-ajustadas al contenido en vez de texto plano.
- **Carga perezosa (lazy) de tablas por base de datos**: expandir una
  conexión antes disparaba un walk completo de todas las tablas de todas
  sus bases (2 queries por tabla) — muy lento en servidores con muchas
  bases. Ahora solo se listan nombres de bases al expandir (1 query), y las
  tablas de una base se cargan recién al abrir su tab (3 queries en batch,
  cacheadas en el tab para la sesión). *(No hay una cifra de mejora medida
  y persistida en el repo para citar; la comparación se hizo con un script
  descartable que nunca se commiteó.)*
- **Búsqueda del sidebar (`/`) scopeada al contexto**: busca nombres de
  base al navegar el árbol de conexiones, y nombres de tabla (solo dentro
  del tab abierto) una vez que una base está abierta — antes mezclaba
  todas las tablas cargadas de todas las conexiones sin importar el contexto.
- **Favoritos ahora son por base de datos, no por tabla**: se renombró
  `TableRef` → `DbRef` (conexión+base, sin tabla). Se puede favoritear
  (`f`) desde el árbol de conexiones, desde un tab abierto, o desde la
  lista pineada misma.

### Corregido
- **Favoritear en el árbol movía el cursor a un nodo equivocado**: al
  presionar `f` sobre una base en el árbol, se insertaba una fila pineada
  arriba y el cursor no se ajustaba, quedando apuntando a otro nodo
  (podía activar el expand de la conexión de arriba en vez de abrir la
  base recién favoriteada). Ahora el cursor sigue al mismo nodo lógico
  tras el shift de la lista.
- **Se eliminó el pineado automático de "recientes"**: el tracking de las
  últimas 10 bases abiertas se pineaba arriba del árbol de conexiones de
  forma indistinguible de los favoritos reales, generando confusión
  (reportado en vivo: "quedan pegados, y no son favoritos"). Se quitó por
  completo esa mecánica; ahora solo queda pineado lo que se marca
  explícitamente con `f`. Se migraron los favoritos reales existentes
  (`rmji/almevet`, `rmji/almevet_20260320`) preservándolos, descartando
  las entradas que eran solo "recientes".
- **Aislamiento de tests**: un test de settings escribía sobre el
  `~/.config/sqldr/config.toml` real del usuario en vez de un directorio
  temporal. Se agregó `SQLDR_CONFIG_DIR` como override solo-test, y un
  test de regresión que prueba que el color de borde enfocado sigue al
  tema activo (no un `Color::Cyan` hardcodeado).

### Traducido
- README y todos los textos visibles de la TUI/CLI (títulos de paneles,
  mensajes de estado y error, wizard, historial, diálogo de opciones) del
  español al inglés.
- Se quitó una referencia colgante a `START_HERE.md` (archivo ya eliminado)
  en un comentario de documentación de `clipboard.rs`.
