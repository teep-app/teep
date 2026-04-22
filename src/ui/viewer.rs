use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{AppState, Focus, OpenFile};

pub fn render(state: &AppState, area: Rect, frame: &mut Frame) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    render_header(state, chunks[0], frame);
    render_body(state, chunks[1], frame);
}

fn render_header(state: &AppState, area: Rect, frame: &mut Frame) {
    let text = match &state.open_file {
        Some(f) => {
            let relative = f.path.strip_prefix(&state.root).unwrap_or(f.path.as_path());
            let marker = if f.error.is_some() {
                " [error]"
            } else if f.diff_mode {
                " [diff vs HEAD]"
            } else {
                ""
            };
            format!(" {}{}", relative.display(), marker)
        }
        None => " (no file open — press Tab then Enter on a file in the tree)".to_string(),
    };
    let focused = state.focus == Focus::Viewer;
    let style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
}

fn render_body(state: &AppState, area: Rect, frame: &mut Frame) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if state.focus == Focus::Viewer {
            Color::Cyan
        } else {
            Color::DarkGray
        }));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(open) = &state.open_file else {
        let hint = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Navigate the tree with arrow keys.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "Enter / o: open file.   n: jump to next change.   Tab: switch pane.",
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        frame.render_widget(hint, inner);
        return;
    };

    if let Some(err) = &open.error {
        let p = Paragraph::new(Line::from(Span::styled(
            format!("could not open: {err}"),
            Style::default().fg(Color::Red),
        )));
        frame.render_widget(p, inner);
        return;
    }

    if open.diff_mode {
        render_diff(open, inner, frame);
        return;
    }

    let gutter_width = gutter_width_for(open);
    let gutter_area = Rect {
        x: inner.x,
        y: inner.y,
        width: gutter_width,
        height: inner.height,
    };
    let content_area = Rect {
        x: inner.x + gutter_width,
        y: inner.y,
        width: inner.width.saturating_sub(gutter_width),
        height: inner.height,
    };

    let total = open.highlighted.len();
    let start = open.scroll.min(total);
    let end = (start + inner.height as usize).min(total);

    let gutter: Vec<Line> = (start..end)
        .map(|i| {
            Line::from(Span::styled(
                format!(
                    "{:>width$} ",
                    i + 1,
                    width = (gutter_width as usize).saturating_sub(1)
                ),
                Style::default().fg(Color::DarkGray),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(gutter), gutter_area);

    let body: Vec<Line> = open.highlighted[start..end].to_vec();
    frame.render_widget(Paragraph::new(body), content_area);
}

fn render_diff(open: &OpenFile, area: Rect, frame: &mut Frame) {
    if let Some(err) = &open.diff_error {
        let p = Paragraph::new(Line::from(Span::styled(
            format!("diff failed: {err}"),
            Style::default().fg(Color::Red),
        )));
        frame.render_widget(p, area);
        return;
    }
    let Some(diff) = &open.diff else {
        let p = Paragraph::new(Line::from(Span::styled(
            "computing diff…",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(p, area);
        return;
    };
    if diff.is_empty() {
        let p = Paragraph::new(Line::from(Span::styled(
            "no changes vs HEAD",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(p, area);
        return;
    }

    use crate::git::DiffLineKind;
    let lines: Vec<Line> = diff
        .iter()
        .skip(open.scroll)
        .take(area.height as usize)
        .map(|dl| {
            let (prefix, style) = match dl.kind {
                DiffLineKind::HunkHeader => (
                    "   ",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                DiffLineKind::Added => (" + ", Style::default().fg(Color::Green)),
                DiffLineKind::Removed => (" - ", Style::default().fg(Color::Red)),
                DiffLineKind::Context => ("   ", Style::default().fg(Color::Gray)),
            };
            Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(dl.content.clone(), style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn gutter_width_for(open: &OpenFile) -> u16 {
    let total = open.highlighted.len().max(1);
    let digits = ((total as f64).log10().floor() as u16) + 1;
    digits + 1 // trailing space
}
