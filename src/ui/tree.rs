use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    app::{AppState, Focus},
    tree::NodeKind,
};

/// Render the tree; return `(absolute_row, path)` pairs for every node
/// actually drawn so the mouse handler can route clicks.
pub fn render(state: &AppState, area: Rect, frame: &mut Frame) -> Vec<(u16, std::path::PathBuf)> {
    let visible = state.tree.visible();
    let height = area.height as usize;

    // Compute scroll so the selected row stays in view.
    let selected_idx = visible
        .iter()
        .position(|(_, n)| n.path == state.tree.selected)
        .unwrap_or(0);
    let scroll = compute_scroll(state.tree.scroll, selected_idx, height);

    let focused = state.focus == Focus::Tree;
    let mut rows: Vec<(u16, std::path::PathBuf)> = Vec::new();
    let lines: Vec<Line> = visible
        .iter()
        .skip(scroll)
        .take(height)
        .enumerate()
        .map(|(i, (depth, node))| {
            rows.push((area.y + i as u16, node.path.clone()));
            let is_sel = node.path == state.tree.selected;
            let icon = match node.kind {
                NodeKind::Dir if node.expanded => "▾ ",
                NodeKind::Dir => "▸ ",
                NodeKind::File => "  ",
            };
            let indent = "  ".repeat(*depth);
            let style = if is_sel {
                if focused {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                }
            } else {
                match node.kind {
                    NodeKind::Dir => Style::default().fg(Color::Blue),
                    NodeKind::File => Style::default(),
                }
            };
            Line::from(vec![
                Span::raw(indent),
                Span::styled(format!("{}{}", icon, node.name), style),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
    rows
}

fn compute_scroll(current_scroll: usize, selected: usize, height: usize) -> usize {
    if height == 0 {
        return 0;
    }
    if selected < current_scroll {
        selected
    } else if selected >= current_scroll + height {
        selected.saturating_sub(height.saturating_sub(1))
    } else {
        current_scroll
    }
}
