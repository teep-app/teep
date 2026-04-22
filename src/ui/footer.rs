use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{AppState, Focus};

pub fn render(state: &AppState, area: Rect, frame: &mut Frame) {
    if state.overlay.is_active() {
        let p = Paragraph::new(Line::from(Span::styled(
            " Esc: dismiss ",
            Style::default().fg(Color::Black).bg(Color::Cyan),
        )));
        frame.render_widget(p, area);
        return;
    }

    if let Some((msg, _)) = &state.status {
        let p = Paragraph::new(Line::from(Span::styled(
            format!(" {msg} "),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        )));
        frame.render_widget(p, area);
        return;
    }

    if state.last_ctrl_c.is_some() {
        let p = Paragraph::new(Line::from(Span::styled(
            " Ctrl-C again to quit ",
            Style::default().fg(Color::Black).bg(Color::Yellow),
        )));
        frame.render_widget(p, area);
        return;
    }

    let unseen = state.changes.unseen_count();
    let hints: Vec<Span> = match state.focus {
        Focus::Tree => vec![
            hint("↑↓", "nav"),
            sep(),
            hint("Enter", "open"),
            sep(),
            hint_unseen(unseen),
            sep(),
            hint("/", "find"),
            sep(),
            hint(":", "cmd"),
            sep(),
            hint("?", "help"),
            sep(),
            hint("r", "refresh"),
            sep(),
            hint("Tab", "viewer"),
            sep(),
            hint("Ctrl-C×2", "quit"),
        ],
        Focus::Viewer => vec![
            hint("↑↓", "scroll"),
            sep(),
            hint("PgUp/PgDn", "page"),
            sep(),
            hint_unseen(unseen),
            sep(),
            hint("/", "find"),
            sep(),
            hint(":", "cmd"),
            sep(),
            hint("?", "help"),
            sep(),
            hint("Tab", "tree"),
            sep(),
            hint("Ctrl-C×2", "quit"),
        ],
    };
    frame.render_widget(Paragraph::new(Line::from(hints)), area);
}

fn hint(key: &'static str, label: &'static str) -> Span<'static> {
    Span::styled(
        format!(" {key}:{label}"),
        Style::default().fg(Color::DarkGray),
    )
}

fn hint_unseen(n: usize) -> Span<'static> {
    if n == 0 {
        Span::styled(" n:no-changes", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(
            format!(" n:{n}-new"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    }
}

fn sep() -> Span<'static> {
    Span::styled("  ", Style::default().fg(Color::DarkGray))
}
