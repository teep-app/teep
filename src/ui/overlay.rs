use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::{
    app::{AppState, Overlay},
    commands::{COMMANDS, PaletteState},
    finder::FinderState,
};

pub fn render(state: &AppState, frame: &mut Frame) {
    let area = centered_rect(frame.area(), 70, 60);

    frame.render_widget(Clear, area);
    match &state.overlay {
        Overlay::Finder(f) => render_finder(f, area, frame),
        Overlay::Palette(p) => render_palette(p, area, frame),
        Overlay::Help => render_help(area, frame),
        Overlay::None => {}
    }
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let w = (area.width * percent_x / 100).min(80);
    let h = (area.height * percent_y / 100).min(24);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn overlay_block(title: &str) -> Block<'_> {
    Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
}

fn render_finder(f: &FinderState, area: Rect, frame: &mut Frame) {
    let block = overlay_block("open file");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    // Input line.
    let input = Paragraph::new(Line::from(vec![
        Span::styled("› ", Style::default().fg(Color::Cyan)),
        Span::raw(&f.query),
        Span::styled("█", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(input, layout[0]);

    // Count line.
    let count = Paragraph::new(Line::from(Span::styled(
        format!(
            " {} match{} of {}",
            f.matches.len(),
            if f.matches.len() == 1 { "" } else { "es" },
            f.items.len()
        ),
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(count, layout[1]);

    // Matches.
    let height = layout[2].height as usize;
    let start = f.selected.saturating_sub(height / 2);
    let lines: Vec<Line> = f
        .matches
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, m)| {
            let display = &f.items[m.index].display;
            let style = if i == f.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(Span::styled(format!(" {display}"), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), layout[2]);

    // Footer hint.
    let hint = Paragraph::new(Line::from(Span::styled(
        " ↑↓ select · Enter open · Esc cancel ",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(hint, layout[3]);
}

fn render_palette(p: &PaletteState, area: Rect, frame: &mut Frame) {
    let block = overlay_block("command");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let input = Paragraph::new(Line::from(vec![
        Span::styled(": ", Style::default().fg(Color::Cyan)),
        Span::raw(&p.query),
        Span::styled("█", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(input, layout[0]);

    let lines: Vec<Line> = p
        .matches
        .iter()
        .enumerate()
        .take(layout[1].height as usize)
        .map(|(i, &cmd_idx)| {
            let cmd = COMMANDS[cmd_idx];
            let row_style = if i == p.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let name_style = if i == p.selected {
                row_style
            } else {
                Style::default().fg(Color::Cyan)
            };
            Line::from(vec![
                Span::styled(format!(" {:<12}", cmd.name), name_style),
                Span::styled(cmd.description.to_string(), row_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), layout[1]);

    let hint = Paragraph::new(Line::from(Span::styled(
        " ↑↓ select · Enter run · Esc cancel ",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(hint, layout[2]);
}

fn render_help(area: Rect, frame: &mut Frame) {
    let block = overlay_block("keybindings");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let row = |key: &'static str, desc: &'static str| {
        Line::from(vec![
            Span::styled(
                format!(" {key:<14}"),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(desc),
        ])
    };

    let lines = vec![
        row("↑ ↓", "move selection (tree) / scroll (viewer)"),
        row("→ ←", "expand / collapse dir"),
        row("Enter, o", "open file or toggle dir"),
        row("Tab", "switch focus between tree and viewer"),
        row("n / N", "jump to next / prev unseen change"),
        row("u", "mark all changes as seen (checkpoint)"),
        row("r", "refresh tree now"),
        row("/", "fuzzy file finder"),
        row(":", "command palette"),
        row("?", "this help"),
        row("Ctrl-B", "toggle sidebar"),
        row("Click", "select / focus"),
        row("Click twice", "open file or toggle dir"),
        row("Scroll", "scroll the pane under the cursor"),
        row("Esc", "dismiss overlay"),
        row("Ctrl-C Ctrl-C", "quit"),
        Line::from(""),
        Line::from(Span::styled(
            " Any key to dismiss.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}
