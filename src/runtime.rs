use std::{path::PathBuf, sync::Arc};

use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use crate::{
    app::{Cmd, LoadedFile, Msg},
    syntax, tree,
};

/// Executes side-effecting `Cmd`s emitted by `update`. Results come back as
/// `Msg`s on the main channel.
#[derive(Clone)]
pub struct Runtime {
    tx: UnboundedSender<Msg>,
    root: PathBuf,
}

impl Runtime {
    pub fn new(tx: UnboundedSender<Msg>, root: PathBuf) -> Self {
        Self { tx, root }
    }

    pub fn execute(&self, cmd: Cmd) {
        match cmd {
            Cmd::LoadFile(path) => self.spawn_load_file(path),
            Cmd::RebuildTree => self.spawn_rebuild_tree(),
        }
    }

    fn spawn_rebuild_tree(&self) {
        let tx = self.tx.clone();
        let root = self.root.clone();
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(move || tree::build_node(&root)).await {
                Ok(Ok(node)) => {
                    let _ = tx.send(Msg::TreeRebuilt(node));
                }
                Ok(Err(e)) => warn!(?e, "tree rebuild failed"),
                Err(e) => warn!(?e, "tree rebuild panicked"),
            }
        });
    }

    fn spawn_load_file(&self, path: PathBuf) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            debug!(?path, "loading file");
            let result = load_and_highlight(path.clone()).await;
            if tx.send(Msg::FileLoaded { path, result }).is_err() {
                warn!("event receiver dropped while loading file");
            }
        });
    }
}

async fn load_and_highlight(path: PathBuf) -> Result<LoadedFile, String> {
    const MAX_BYTES: u64 = 2 * 1024 * 1024; // 2 MB ceiling for highlighting

    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|e| e.to_string())?;
    if metadata.len() > MAX_BYTES {
        return Err(format!(
            "file is {} bytes — over the {} byte ceiling for highlighted view",
            metadata.len(),
            MAX_BYTES
        ));
    }
    let text = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| e.to_string())?;

    let text_for_blocking = text.clone();
    let path_for_blocking = path.clone();
    let highlighted = tokio::task::spawn_blocking(move || {
        syntax::highlight_file(&text_for_blocking, &path_for_blocking)
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(LoadedFile {
        text,
        highlighted: Arc::new(highlighted),
    })
}
