//! Mermaid diagram rendering via the external `mmdc` CLI.
//!
//! Pipeline:
//! 1. Hash the (source, theme) pair into a stable cache key.
//! 2. If `<cache_dir>/<hash>.png` already exists, use it directly.
//! 3. Otherwise spawn `mmdc` with source written to a temp file, capture
//!    the PNG into the cache.
//!
//! When `mmdc` isn't on `$PATH`, callers fall back to a bordered-source
//! placeholder and Teep stays honest about the dependency.

use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::OnceLock,
};

use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;

/// Default theme passed to `mmdc`. Static for V1; a config surface
/// arrives with the broader theming pass.
pub const DEFAULT_THEME: &str = "dark";

/// Pixel width we ask `mmdc` to render at. Big enough to look crisp when
/// scaled down into the preview pane and to stay sharp on Retina
/// terminals; small enough that PNGs don't bloat the cache. Included in
/// the cache key so a future bump auto-invalidates stale entries.
const RENDER_WIDTH_PX: u32 = 2400;

static MMDC_PRESENT: OnceLock<bool> = OnceLock::new();

/// True when `mmdc` is reachable on the current `$PATH`. Result is
/// cached for the lifetime of the process so the probe only runs once.
pub fn is_available() -> bool {
    *MMDC_PRESENT.get_or_init(|| {
        Command::new("mmdc")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Cache directory: `<ProjectDirs("teep").cache_dir()>/mermaid/`. Not
/// created here; `render` handles that on demand.
pub fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "teep").map(|d| d.cache_dir().join("mermaid"))
}

/// Content-hash of `(source, theme, render width)` — deterministic across
/// runs and across machines, which is what makes the on-disk cache safe.
/// Including the render width means bumping `RENDER_WIDTH_PX` silently
/// invalidates stale small PNGs without a manual cache wipe.
pub fn cache_key(source: &str, theme: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(source.as_bytes());
    h.update(b"|");
    h.update(theme.as_bytes());
    h.update(b"|w=");
    h.update(RENDER_WIDTH_PX.to_string().as_bytes());
    h.finalize().to_hex().to_string()
}

/// If a rendered PNG already exists for `hash`, return its path.
pub fn cached_path(hash: &str) -> Option<PathBuf> {
    let dir = cache_dir()?;
    let path = dir.join(format!("{hash}.png"));
    path.try_exists().ok().and_then(|e| e.then_some(path))
}

/// Render a mermaid diagram by shelling out to `mmdc`. CPU/IO-bound;
/// callers must run this on a blocking task. Idempotent: if the cache
/// file already exists, returns it immediately without respawning.
pub fn render(source: &str, theme: &str) -> Result<PathBuf> {
    let dir = cache_dir().ok_or_else(|| anyhow!("no cache directory"))?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let hash = cache_key(source, theme);
    let out = dir.join(format!("{hash}.png"));
    if out.try_exists().unwrap_or(false) {
        return Ok(out);
    }

    // `mmdc -i -` (stdin) is documented but brittle in newer CLI
    // versions; writing a neighbour temp file is boring and reliable.
    let input_path = dir.join(format!("{hash}.mmd"));
    std::fs::write(&input_path, source.as_bytes())
        .with_context(|| format!("writing mmd input at {}", input_path.display()))?;

    let output = Command::new("mmdc")
        .arg("-i")
        .arg(&input_path)
        .arg("-o")
        .arg(&out)
        .arg("-t")
        .arg(theme)
        .arg("-b")
        .arg("transparent")
        .arg("-w")
        .arg(RENDER_WIDTH_PX.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("spawning mmdc")?;

    // Best-effort cleanup of the input file, regardless of success.
    let _ = std::fs::remove_file(&input_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = stderr.trim();
        let extra = stdout.trim();
        let combined = if extra.is_empty() {
            detail.to_string()
        } else {
            format!("{detail} · {extra}")
        };
        let msg = if combined.is_empty() {
            format!("mmdc exited {}", output.status)
        } else {
            format!("mmdc exited {}: {}", output.status, combined)
        };
        anyhow::bail!(msg);
    }

    if !out.try_exists().unwrap_or(false) {
        anyhow::bail!("mmdc succeeded but output {} is missing", out.display());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_across_calls() {
        let a = cache_key("graph LR\nA-->B", "dark");
        let b = cache_key("graph LR\nA-->B", "dark");
        assert_eq!(a, b);
    }

    #[test]
    fn cache_key_differs_per_source() {
        let a = cache_key("graph LR\nA-->B", "dark");
        let b = cache_key("graph LR\nA-->C", "dark");
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_differs_per_theme() {
        let a = cache_key("graph LR\nA-->B", "dark");
        let b = cache_key("graph LR\nA-->B", "light");
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_is_hex() {
        let k = cache_key("x", "dark");
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(k.len(), 64, "blake3 hex is 64 chars");
    }
}
