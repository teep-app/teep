mod app;
mod changes;
mod commands;
mod config;
mod event;
mod finder;
mod fs_watch;
mod git;
mod image;
mod markdown;
mod runtime;
mod syntax;
mod tree;
mod ui;

use std::{
    io::{self, Stdout, stdout},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "teep",
    version,
    about = "Your private telepath for the agent writing your code."
)]
struct Cli {
    /// Repository root (defaults to current directory).
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = config::load().context("loading config")?;
    let _log_guard = config::init_logging().context("initializing logging")?;

    let root = match cli.path {
        Some(p) => p
            .canonicalize()
            .with_context(|| format!("canonicalizing {}", p.display()))?,
        None => std::env::current_dir().context("getting current directory")?,
    };
    info!(?root, "starting teep");

    // Probe the terminal for its graphics-protocol capability BEFORE we go
    // into raw mode + alt screen. The query sends escape sequences to stdout
    // and reads the response from stdin; once crossterm's event loop owns
    // stdin the response is unreachable and we'd silently fall back to
    // halfblocks (very pixelated image output).
    image::init_early();

    install_panic_hook();
    let mut terminal = setup_terminal().context("setting up terminal")?;

    let result = app::run(&mut terminal, root, config).await;

    if let Err(e) = restore_terminal(&mut terminal) {
        eprintln!("failed to restore terminal: {e}");
    }
    result
}

fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal_raw();
        original_hook(info);
    }));
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
    )?;
    Ok(Terminal::new(CrosstermBackend::new(stdout()))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    restore_terminal_raw()?;
    terminal.show_cursor()?;
    Ok(())
}

fn restore_terminal_raw() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        stdout(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
    )?;
    Ok(())
}
