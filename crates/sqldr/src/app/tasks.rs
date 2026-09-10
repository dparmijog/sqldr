//! Small helpers shared by the background `tokio::spawn` calls that fit
//! their shape, cutting down the "clone the event sender, spawn, match
//! the result, send an event either way" boilerplate that was duplicated
//! across query.rs/wizard.rs.
//!
//! Not a universal fit: `connect_and_load_schema`'s multi-step chain
//! (each step fires its own intermediate event on the way to the next)
//! and `start_heartbeat`'s infinite ping loop have genuinely different
//! control flow from "one async operation, one terminal event" — they're
//! deliberately left as hand-rolled `tokio::spawn` calls rather than
//! forced through an abstraction that wouldn't actually fit them.

use std::future::Future;

use futures::{Stream, StreamExt};
use sqldr_core::Row;
use tokio::sync::mpsc;

use super::{App, AppEvent};

impl App {
    /// Spawns `fut`, sending whatever `AppEvent` `make_event` builds from
    /// its `Result` once it resolves. Covers every background task that
    /// reduces to "one async operation, one terminal event either way" —
    /// a live credential test, fetching one database's tables.
    pub(super) fn spawn_into_event<T, Fut>(
        &self,
        fut: Fut,
        make_event: impl FnOnce(Result<T, String>) -> AppEvent + Send + 'static,
    ) where
        Fut: Future<Output = Result<T, String>> + Send + 'static,
        T: Send + 'static,
    {
        let tx = self.events.clone();
        tokio::spawn(async move {
            let _ = tx.send(make_event(fut.await));
        });
    }
}

/// Streams `stream` into the results pane as `AppEvent::QueryRow`,
/// followed by `QueryDone` — or `QueryError` (which stops the pump early)
/// on the first `Err`. Shared by a live streamed query and a
/// fully-buffered `EXPLAIN` plan (wrapped via `futures::stream::iter`) —
/// both ultimately fill the results pane the exact same way.
pub(super) async fn pump_rows(
    tx: &mpsc::UnboundedSender<AppEvent>,
    mut stream: impl Stream<Item = anyhow::Result<Row>> + Unpin,
) {
    while let Some(item) = stream.next().await {
        match item {
            Ok(row) => {
                if tx.send(AppEvent::QueryRow(row)).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(AppEvent::QueryError(e.to_string()));
                return;
            }
        }
    }
    let _ = tx.send(AppEvent::QueryDone);
}
