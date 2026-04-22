use std::path::PathBuf;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::{app::AppState, ui::tree as tree_ui};

pub struct SidebarRows {
    pub tree_rows: Vec<(u16, PathBuf)>,
    pub change_rows: Vec<(u16, PathBuf)>,
}

pub fn render(state: &AppState, area: Rect, frame: &mut Frame) -> SidebarRows {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Max(10),
        ])
        .split(area);

    let tree_rows = tree_ui::render(state, chunks[0], frame);

    let seen = state.changes.unseen_count();
    let total = state.changes.entries().len();
    let header = format!(
        " changes: {}/{}{} ",
        seen,
        total,
        if seen > 0 { "  (press n)" } else { "" }
    );
    let header_style = if seen > 0 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let divider = Paragraph::new(Line::from(Span::styled(header, header_style)));
    frame.render_widget(divider, chunks[1]);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let change_area = chunks[2];
    let inner = block.inner(change_area);

    // Collect entries newest-first, keep track of the path so mouse clicks
    // can resolve back to it.
    let entries: Vec<_> = state
        .changes
        .entries()
        .iter()
        .rev()
        .take(inner.height as usize)
        .collect();

    let mut change_rows: Vec<(u16, PathBuf)> = Vec::new();
    let change_lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .map(|(i, c)| {
            change_rows.push((inner.y + i as u16, c.path.clone()));
            let relative = c.path.strip_prefix(&state.root).unwrap_or(c.path.as_path());
            let marker = if c.seen_by_user { " " } else { "*" };
            let style = if c.seen_by_user {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Yellow)
            };
            Line::from(Span::styled(
                format!("{marker} {}", relative.display()),
                style,
            ))
        })
        .collect();

    frame.render_widget(Paragraph::new(change_lines).block(block), change_area);

    SidebarRows {
        tree_rows,
        change_rows,
    }
}
