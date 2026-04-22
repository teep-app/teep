use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{DebounceEventResult, Debouncer, FileIdMap, new_debouncer};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use crate::app::Msg;

/// Keeps the debouncer alive for as long as this struct is alive.
/// Dropping it stops the watcher.
pub struct FsWatcher {
    _debouncer: Debouncer<notify::RecommendedWatcher, FileIdMap>,
}

pub fn spawn(root: PathBuf, tx: UnboundedSender<Msg>) -> Result<FsWatcher> {
    let tx_cb = tx.clone();
    let mut debouncer = new_debouncer(
        Duration::from_millis(100),
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) => {
                let mut paths: Vec<PathBuf> = events
                    .into_iter()
                    .flat_map(|e| e.event.paths.clone())
                    .collect();
                paths.sort();
                paths.dedup();
                if !paths.is_empty() {
                    let _ = tx_cb.send(Msg::FsChanged(paths));
                }
            }
            Err(errs) => {
                for e in errs {
                    warn!(?e, "fs watcher error");
                }
            }
        },
    )
    .context("creating fs debouncer")?;

    debouncer
        .watcher()
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", root.display()))?;

    debug!(?root, "fs watcher started");
    Ok(FsWatcher {
        _debouncer: debouncer,
    })
}
