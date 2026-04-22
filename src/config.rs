use std::{fs, path::PathBuf};

use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, prelude::*};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "base16-ocean.dark".to_string(),
        }
    }
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "hitled")
}

fn config_path() -> Option<PathBuf> {
    Some(project_dirs()?.config_dir().join("config.toml"))
}

fn log_dir() -> Option<PathBuf> {
    Some(project_dirs()?.data_local_dir().to_path_buf())
}

pub fn load() -> Result<Config> {
    let Some(path) = config_path() else {
        return Ok(Config::default());
    };
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing config at {}", path.display()))
}

pub fn init_logging() -> Result<WorkerGuard> {
    let dir = log_dir().ok_or_else(|| anyhow!("could not determine log directory"))?;
    fs::create_dir_all(&dir).with_context(|| format!("creating log dir {}", dir.display()))?;

    let file_appender = tracing_appender::rolling::never(&dir, "hitled.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_env("HITLED_LOG").unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_target(false),
        )
        .init();

    Ok(guard)
}
