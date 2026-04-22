use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{AppState, EditState, Focus, InlineImageState, OpenFile};
use crate::markdown::InlineImageRef;

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
            } else if f.image.is_some() {
                " [image]"
            } else if f.image_error.is_some() {
                " [image · failed]"
            } else if matches!(f.edit, EditState::Deleted { .. }) {
                " [file removed]"
            } else if let EditState::Edit(b) = &f.edit {
                match (b.is_live_preview(), b.is_dirty()) {
                    (true, true) => " [live · unsaved]",
                    (true, false) => " [live preview]",
                    (false, true) => " [edit · unsaved]",
                    (false, false) => " [edit]",
                }
            } else if matches!(f.edit, EditState::Conflict { .. }) {
                " [edit · conflict]"
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

    // M7: image files render as pictures, not as text.
    if let Some(err) = &open.image_error {
        let p = Paragraph::new(Line::from(Span::styled(
            format!("could not decode image: {err}"),
            Style::default().fg(Color::Red),
        )));
        frame.render_widget(p, inner);
        return;
    }
    if let Some(image_cell) = &open.image {
        render_image(image_cell, inner, frame);
        return;
    }

    match &open.edit {
        EditState::Edit(buffer) => {
            render_edit(buffer, inner, frame);
            return;
        }
        EditState::Conflict { buffer } => {
            render_conflict(buffer, inner, frame);
            return;
        }
        EditState::Deleted { buffer } => {
            render_deleted(buffer, inner, frame);
            return;
        }
        EditState::View => {}
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

fn render_image(
    image_cell: &std::cell::RefCell<ratatui_image::protocol::StatefulProtocol>,
    area: Rect,
    frame: &mut Frame,
) {
    // `StatefulImage<T>` is generic over the protocol kind; we use
    // `StatefulProtocol`. We keep the protocol in a `RefCell` because our
    // render path has only `&AppState`.
    let mut protocol = image_cell.borrow_mut();
    let widget = ratatui_image::StatefulImage::<ratatui_image::protocol::StatefulProtocol>::new();
    frame.render_stateful_widget(widget, area, &mut *protocol);
}

fn render_edit(buffer: &crate::app::EditBuffer, area: Rect, frame: &mut Frame) {
    if buffer.live_blocks.is_some() {
        render_live_preview(buffer, area, frame);
    } else {
        frame.render_widget(&buffer.textarea, area);
    }
}

/// Render a markdown buffer in Live Preview mode: the block whose source
/// range contains the cursor renders as raw text (so you can see `##`,
/// `**`, `|`); every other block renders cooked. Blank lines between
/// blocks render in their source position so cursor coordinates stay
/// honest when the cursor is parked on an empty line.
///
/// Inline images: sole-image paragraphs reserve `INLINE_IMAGE_ROWS` blank
/// lines in the text pass, then a second pass overlays the actual
/// `StatefulImage` widget on that rect. When the cursor is on an image
/// block, it falls into the raw-source branch and no overlay happens —
/// consistent with Level-B reveal-on-cursor.
fn render_live_preview(buffer: &crate::app::EditBuffer, area: Rect, frame: &mut Frame) {
    struct ImageOverlay<'a> {
        visual_start: usize,
        image: &'a InlineImageRef,
    }

    let blocks = buffer
        .live_blocks
        .as_ref()
        .expect("called with live blocks");
    let (cursor_row, cursor_col) = buffer.textarea.cursor();
    let current_block_idx = crate::markdown::block_at_row(blocks, cursor_row);
    let lines = buffer.textarea.lines();
    let total_rows = lines.len();

    let mut visual: Vec<Line<'static>> = Vec::new();
    let mut cursor_visual: Option<(usize, usize)> = None;
    let mut image_overlays: Vec<ImageOverlay> = Vec::new();

    let mut row = 0;
    while row < total_rows {
        if let Some(bi) = crate::markdown::block_at_row(blocks, row) {
            let block = &blocks[bi];
            if Some(bi) == current_block_idx {
                // Raw — one visual line per source line, cursor tracked.
                for src_row in block.source_start..block.source_end {
                    if let Some(line_text) = lines.get(src_row) {
                        if src_row == cursor_row {
                            cursor_visual = Some((visual.len(), cursor_col));
                        }
                        visual.push(Line::from(Span::raw(line_text.clone())));
                    }
                }
            } else {
                // Cooked — emit the pre-rendered block lines in place.
                let block_visual_start = visual.len();
                for cooked in &block.cooked {
                    visual.push(cooked.clone());
                }
                if let Some(img) = &block.image {
                    image_overlays.push(ImageOverlay {
                        visual_start: block_visual_start,
                        image: img,
                    });
                }
            }
            row = block.source_end;
        } else {
            // Inter-block or trailing blank line. Render empty, but track the
            // cursor if it's parked here so it sits where the user expects.
            if row == cursor_row {
                cursor_visual = Some((visual.len(), cursor_col));
            }
            visual.push(Line::from(""));
            row += 1;
        }
    }

    // If the cursor is somehow past the last row (defensive — shouldn't
    // happen, tui-textarea keeps the cursor in range), leave it alone.

    // Scroll so the cursor row is inside the viewport.
    let height = area.height as usize;
    let cursor_row_visual = cursor_visual.map(|(r, _)| r).unwrap_or(0);
    let scroll = if height == 0 || cursor_row_visual < height {
        0
    } else {
        cursor_row_visual + 1 - height
    };
    let total = visual.len();
    let start = scroll.min(total);
    let end = (start + height).min(total);
    let body: Vec<Line> = visual[start..end].to_vec();
    frame.render_widget(Paragraph::new(body), area);

    // Second pass: overlay each image block whose reserved rect intersects
    // the viewport. Lookups against the decode cache — render the actual
    // picture, a decoding spinner, or an error hint accordingly.
    for overlay in &image_overlays {
        let block_end_visual = overlay.visual_start + overlay.image.height_cells as usize;
        if overlay.visual_start >= end || block_end_visual <= start {
            continue;
        }
        let clipped_start = overlay.visual_start.max(start);
        let clipped_end = block_end_visual.min(end);
        let rect = Rect {
            x: area.x,
            y: area.y + (clipped_start - start) as u16,
            width: area.width,
            height: (clipped_end - clipped_start) as u16,
        };
        match buffer.inline_images.get(&overlay.image.path) {
            Some(InlineImageState::Loaded(cell)) => {
                let mut protocol = cell.borrow_mut();
                let widget = ratatui_image::StatefulImage::<
                    ratatui_image::protocol::StatefulProtocol,
                >::new();
                frame.render_stateful_widget(widget, rect, &mut *protocol);
            }
            Some(InlineImageState::Loading) => {
                let name = overlay
                    .image
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| overlay.image.path.display().to_string());
                let p = Paragraph::new(Line::from(Span::styled(
                    format!("  decoding {name}…"),
                    Style::default().fg(Color::DarkGray),
                )));
                frame.render_widget(p, rect);
            }
            Some(InlineImageState::Failed(e)) => {
                let label = if overlay.image.alt.is_empty() {
                    format!("  [image · {e}]")
                } else {
                    format!("  [image: {} · {}]", overlay.image.alt, e)
                };
                let p = Paragraph::new(Line::from(Span::styled(
                    label,
                    Style::default().fg(Color::Red),
                )));
                frame.render_widget(p, rect);
            }
            None => {}
        }
    }

    if let Some((vrow, vcol)) = cursor_visual
        && vrow >= start
        && vrow < end
    {
        let x = area
            .x
            .saturating_add(vcol as u16)
            .min(area.x + area.width.saturating_sub(1));
        let y = area.y.saturating_add((vrow - start) as u16);
        frame.set_cursor_position((x, y));
    }
}

/// Narrow-pane threshold: below this width the banner collapses to its
/// short form. Relevant keybindings are always surfaced redundantly in
/// the footer, so the banner can safely shrink.
const NARROW_BANNER_WIDTH: u16 = 60;

fn banner<'a>(text: &'a str, fg: Color, bg: Color) -> Paragraph<'a> {
    Paragraph::new(Line::from(Span::styled(
        format!(" {text} "),
        Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    )))
}

fn render_conflict(buffer: &crate::app::EditBuffer, area: Rect, frame: &mut Frame) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    let msg = if area.width < NARROW_BANNER_WIDTH {
        "⚠ conflict on disk"
    } else {
        "⚠ agent modified this file on disk · [k]eep mine · [t]heirs · Esc to keep"
    };
    frame.render_widget(banner(msg, Color::Black, Color::Yellow), split[0]);
    frame.render_widget(&buffer.textarea, split[1]);
}

fn render_deleted(buffer: &crate::app::EditBuffer, area: Rect, frame: &mut Frame) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    let msg = if area.width < NARROW_BANNER_WIDTH {
        "⚠ file removed"
    } else {
        "⚠ file removed on disk · [r]estore from buffer · [c]lose"
    };
    frame.render_widget(banner(msg, Color::White, Color::Red), split[0]);
    frame.render_widget(&buffer.textarea, split[1]);
}

fn gutter_width_for(open: &OpenFile) -> u16 {
    let total = open.highlighted.len().max(1);
    let digits = ((total as f64).log10().floor() as u16) + 1;
    digits + 1 // trailing space
}
