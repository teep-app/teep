mod footer;
mod overlay;
mod sidebar;
mod tree;
mod viewer;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{AppState, MouseLayout};

pub fn view(state: &mut AppState, frame: &mut Frame) {
    let root_area = frame.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer
        ])
        .split(root_area);

    render_header(state, outer[0], frame);

    let body_width = outer[1].width;
    let show_sidebar = state.sidebar_visible && body_width >= 70;
    let sidebar_width: u16 = if body_width < 90 { 24 } else { 30 };

    let mut layout = MouseLayout::default();
    if show_sidebar {
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(sidebar_width), Constraint::Min(1)])
            .split(outer[1]);
        let rows = sidebar::render(state, body[0], frame);
        layout.tree_rows = rows.tree_rows;
        layout.change_rows = rows.change_rows;
        layout.viewer = body[1];
        layout.viewer_col_min = body[1].x;
        viewer::render(state, body[1], frame);
    } else {
        layout.viewer = outer[1];
        layout.viewer_col_min = outer[1].x;
        viewer::render(state, outer[1], frame);
    }
    state.mouse_layout = layout;

    footer::render(state, outer[2], frame);

    if state.overlay.is_active() {
        overlay::render(state, frame);
    }
}

fn render_header(state: &AppState, area: ratatui::layout::Rect, frame: &mut Frame) {
    let root_label = state
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let unseen = state.changes.unseen_count();

    let mut spans: Vec<Span<'_>> = vec![Span::styled(
        " hitled ",
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];

    // Git dot + branch, if we have a snapshot.
    if let Some(snap) = state.git_snapshot.as_ref() {
        let conflict = snap
            .status
            .iter()
            .any(|s| matches!(s.kind, crate::git::StatusKind::Conflicted));
        let dot_color = if conflict {
            Color::Red
        } else if snap.is_clean {
            Color::DarkGray
        } else {
            Color::Yellow
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled("●", Style::default().fg(dot_color)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            snap.branch.clone().unwrap_or_else(|| "(detached)".into()),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        // If the session root is not itself the worktree root, indicate.
        if let Some(wt_name) = snap.worktree_path.file_name().and_then(|s| s.to_str()) {
            spans.push(Span::styled(
                format!(" · wt:{wt_name}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        // Count files the git dot is actually reacting to, so the dot's
        // color has a readable partner.
        let dirty_count = snap
            .status
            .iter()
            .filter(|s| !matches!(s.kind, crate::git::StatusKind::Ignored))
            .count();
        if dirty_count > 0 {
            spans.push(Span::styled(
                format!(" · {dirty_count} modified"),
                Style::default().fg(Color::Yellow),
            ));
        }
    } else {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            root_label,
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        state.root.display().to_string(),
        Style::default().fg(Color::DarkGray),
    ));
    spans.push(Span::raw("  "));
    spans.push(if unseen == 0 {
        Span::styled("all reviewed", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(
            format!("● {unseen} to review"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    });

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
