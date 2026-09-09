# Ideas / Roadmap

Backlog of potential improvements, captured 2026-09-09. Not commitments —
just a prioritized list to pull from. Items marked **[done]** shipped the
same day; the rest stay here for later.

## High value, low effort (reuse existing state/backend)

1. **[done, `Ctrl+X`]** Wire `EXPLAIN` to a key. `Driver::explain()` was
   already implemented for MySQL (`mysql.rs`) but nothing in the TUI ever
   called it — dead code. `Ctrl+X` now runs `EXPLAIN <query>` and shows
   the plan in the results pane, bypassing the mutation guard/`WHERE`
   confirmation entirely since `EXPLAIN` never touches data.
2. **[done, `e`/`d`]** Edit/delete connections from the TUI. Previously
   you could only *add* a connection (`Ctrl+N`); changing host/user/
   password or removing one required hand-editing `config.toml` (and the
   keyring for passwords). `e` on a connection reopens the wizard
   pre-filled for editing (falls back to the already-stored password for
   the live test if the field is left blank); `d` deletes it after
   confirmation, clearing config, keyring, and any open tabs.
3. **Search/filter inside results.** `/` searches the sidebar
   (databases/tables), but once a query has run there's no way to
   filter/search within the results grid itself without rewriting SQL.
4. **[done]** Connection health indicator. The sidebar showed
   `idle`/`connecting`/`connected`/`error`, but there was no periodic
   ping — a connection that silently dropped (server-side timeout) was
   only noticed on the next query. A background `SELECT 1` heartbeat
   every 20s now flips status to `Error` on its own, and a live
   connection shows `(Ns)` since its last successful ping in the sidebar.

## Medium value, medium effort

5. **Second/third engine (Postgres, SQLite).** Literally the stated
   roadmap in the README. `Driver`/`Dialect` are already designed for
   this without touching the TUI; it's the biggest gap between "promised"
   and "delivered" today.
6. **Autocomplete in the editor.** Table/column names from the already
   loaded `Schema`/`Table` state; doesn't need real SQL parsing, a
   prefix-match over known identifiers already helps a lot.
7. **Persist open tabs between sessions.** Every launch starts fresh from
   the connection tree. Explicit "restore session" (not to be confused
   with the removed implicit "recents" pinning) would save real daily
   friction for a fixed set of frequently-used databases.
8. **Export results to a file.** `y`/`Y`/`c`/`i` copy a cell/row via OSC
   52, but there's no way to dump a full result set to `.csv`/`.json` on
   disk for datasets too large for the visible buffer.

## Longer-term / architectural

9. **[done, `s`]** Table structure explorer. `s` on a table row in an
   open tab shows a read-only view of its columns (type/nullability/key),
   indexes, and foreign keys — all already present on the loaded `Table`,
   so it needs no extra query.
10. **[done, `g`]** Foreign-key navigation. `g` on a cell in the results
    grid that's a FK jumps to the referenced row (`SELECT * FROM ref_table
    WHERE ref_col = <value>`). Needs a table preview (`source_table` set)
    since that's where the FK metadata comes from; doesn't work from an
    arbitrary hand-typed query.
11. **SSH tunnel / bastion support.** `ConnConfig` currently assumes
    direct connectivity via `url`; real-world setups often need a tunnel.
12. **Explicit transaction handling.** `BEGIN`/`COMMIT`/`ROLLBACK` tracked
    and surfaced by the UI (visible "you're in an open transaction"
    indicator), instead of leaving it as plain SQL with no visual
    feedback of state.
