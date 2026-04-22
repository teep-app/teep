use std::{path::PathBuf, sync::Arc};

use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use crate::{
    app::{Cmd, LoadedFile, Msg},
    git, image, syntax, tree,
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
            Cmd::SaveFile { path, content } => self.spawn_save_file(path, content),
            Cmd::RebuildTree => self.spawn_rebuild_tree(),
            Cmd::RefreshGit => self.spawn_refresh_git(),
            Cmd::ComputeDiff(path) => self.spawn_compute_diff(path),
            Cmd::ReRoot(new_root) => {
                // Just echo the request back to update() — app::run_session
                // observes this and returns SessionOutcome::Reroot.
                let _ = self.tx.send(Msg::ReRootRequested(new_root));
            }
        }
    }

    fn spawn_save_file(&self, path: PathBuf, content: String) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            debug!(?path, bytes = content.len(), "saving file");
            let result = tokio::fs::write(&path, content.as_bytes())
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::FileSaved { path, result });
        });
    }

    fn spawn_refresh_git(&self) {
        let tx = self.tx.clone();
        let root = self.root.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || git::snapshot(&root)).await;
            let msg = match result {
                Ok(Ok(snap)) => Msg::GitRefreshed(Ok(snap)),
                Ok(Err(e)) => Msg::GitRefreshed(Err(e.to_string())),
                Err(e) => Msg::GitRefreshed(Err(format!("git snapshot task panicked: {e}"))),
            };
            let _ = tx.send(msg);
        });
    }

    fn spawn_compute_diff(&self, path: PathBuf) {
        let tx = self.tx.clone();
        let root = self.root.clone();
        tokio::spawn(async move {
            let path_for_task = path.clone();
            let result =
                tokio::task::spawn_blocking(move || git::diff_vs_head(&root, &path_for_task)).await;
            let result = match result {
                Ok(Ok(lines)) => Ok(lines),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(format!("diff task panicked: {e}")),
            };
            let _ = tx.send(Msg::DiffReady { path, result });
        });
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
        // Image files skip the syntect/text pipeline entirely.
        if image::is_image_path(&path) {
            self.spawn_load_image(path);
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            debug!(?path, "loading file");
            let result = load_and_highlight(path.clone()).await;
            if tx.send(Msg::FileLoaded { path, result }).is_err() {
                warn!("event receiver dropped while loading file");
            }
        });
    }

    fn spawn_load_image(&self, path: PathBuf) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            debug!(?path, "decoding image");
            let path_for_task = path.clone();
            let result =
                tokio::task::spawn_blocking(move || image::decode_image(&path_for_task)).await;
            let result = match result {
                Ok(Ok(img)) => Ok(img),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(format!("decode task panicked: {e}")),
            };
            let _ = tx.send(Msg::ImageLoaded { path, result });
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
