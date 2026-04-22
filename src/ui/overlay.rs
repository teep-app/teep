use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::{
    app::{AppState, Overlay, WorktreeSwitcherState},
    commands::{COMMANDS, PaletteState},
    finder::FinderState,
    git::{GitSnapshot, StatusKind},
};

pub fn render(state: &AppState, frame: &mut Frame) {
    let area = centered_rect(frame.area(), 70, 60);

    frame.render_widget(Clear, area);
    match &state.overlay {
        Overlay::Finder(f) => render_finder(f, area, frame),
        Overlay::Palette(p) => render_palette(p, area, frame),
        Overlay::Help => render_help(area, frame),
        Overlay::GitStatus => render_git_status(state.git_snapshot.as_ref(), area, frame),
        Overlay::WorktreeSwitcher(w) => render_worktree_switcher(w, area, frame),
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

fn render_git_status(snap: Option<&GitSnapshot>, area: Rect, frame: &mut Frame) {
    let block = overlay_block("git status");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(snap) = snap else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " not a git repository",
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
        return;
    };

    let header = |s: &'static str| {
        Line::from(Span::styled(
            format!(" {s}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let kv = |k: &'static str, v: String| {
        Line::from(vec![
            Span::styled(format!(" {k:<10}"), Style::default().fg(Color::DarkGray)),
            Span::raw(v),
        ])
    };

    let mut lines = Vec::new();
    lines.push(header("branch"));
    lines.push(kv(
        "name",
        snap.branch.clone().unwrap_or_else(|| "(detached)".into()),
    ));
    lines.push(kv(
        "head",
        snap.head_short.clone().unwrap_or_else(|| "-".into()),
    ));
    lines.push(kv(
        "clean",
        if snap.is_clean {
            "yes".into()
        } else {
            "no".into()
        },
    ));
    lines.push(Line::from(""));

    lines.push(header("worktrees"));
    for w in &snap.worktrees {
        let marker = if w.is_current { "●" } else { " " };
        let branch = w.branch.clone().unwrap_or_else(|| "(detached)".into());
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {marker} "),
                Style::default().fg(if w.is_current {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }),
            ),
            Span::raw(format!("{}  ", w.path.display())),
            Span::styled(branch, Style::default().fg(Color::Yellow)),
        ]));
    }
    lines.push(Line::from(""));

    lines.push(header("local branches"));
    for b in &snap.branches {
        lines.push(Line::from(Span::raw(format!("   {b}"))));
    }
    lines.push(Line::from(""));

    lines.push(header("status"));
    if snap.status.is_empty() {
        lines.push(Line::from(Span::styled(
            "   (working tree clean)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for entry in &snap.status {
            let color = match entry.kind {
                StatusKind::Modified => Color::Yellow,
                StatusKind::Added => Color::Green,
                StatusKind::Deleted => Color::Red,
                StatusKind::Renamed => Color::Magenta,
                StatusKind::Untracked => Color::DarkGray,
                StatusKind::Ignored => Color::DarkGray,
                StatusKind::Conflicted => Color::Red,
            };
            let rel = entry
                .path
                .strip_prefix(&snap.worktree_path)
                .unwrap_or(entry.path.as_path());
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} ", entry.kind.glyph()),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(rel.display().to_string()),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Esc / any key to dismiss.",
        Style::default().fg(Color::DarkGray),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_worktree_switcher(w: &WorktreeSwitcherState, area: Rect, frame: &mut Frame) {
    let block = overlay_block("switch worktree");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let lines: Vec<Line> = w
        .worktrees
        .iter()
        .enumerate()
        .take(layout[0].height as usize)
        .map(|(i, entry)| {
            let selected = i == w.selected;
            let row_style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let branch = entry.branch.clone().unwrap_or_else(|| "(detached)".into());
            let marker = if entry.is_current { "●" } else { " " };
            Line::from(vec![
                Span::styled(format!(" {marker} "), row_style),
                Span::styled(format!("{}  ", entry.path.display()), row_style),
                Span::styled(branch, row_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), layout[0]);

    let hint = Paragraph::new(Line::from(Span::styled(
        " ↑↓ select · Enter switch · Esc cancel ",
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(hint, layout[1]);
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
        row("d", "toggle diff vs HEAD for current file"),
        row("g", "git status overlay"),
        row("b", "switch git worktree"),
        row("i, e", "enter edit mode; Esc exits, Ctrl-S saves"),
        row("k / t", "(conflict) keep mine / take theirs"),
        row("r / c", "(deleted) restore buffer / close"),
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
