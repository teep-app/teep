use comrak::{
    Arena, Options,
    nodes::{AstNode, ListType, NodeCodeBlock, NodeValue, TableAlignment},
    parse_document,
};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::syntax;

/// Parse `text` as GFM markdown and produce a flat, pre-styled line sequence
/// ready for rendering in a `Paragraph`-style widget. Retained for tests and
/// any future Reading-View (fully-cooked, read-only) surface; the interactive
/// viewer now uses the per-block `live::parse_blocks` instead.
#[allow(dead_code)]
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let arena = Arena::new();
    let options = build_options();
    let root = parse_document(&arena, text, &options);

    let mut out = Vec::new();
    for (i, child) in root.children().enumerate() {
        if i > 0 {
            out.push(Line::from(""));
        }
        render_block(child, &mut out, 0);
    }
    out
}

pub(crate) fn build_options<'a>() -> Options<'a> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;
    options.extension.footnotes = true;
    options.parse.smart = false;
    options
}

/// Render a single top-level AST block into its cooked line sequence.
/// Used by the live-preview module to compose per-block output with
/// source-range metadata.
pub(crate) fn render_block_to_lines<'a>(node: &'a AstNode<'a>) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    render_block(node, &mut out, 0);
    out
}

fn render_block<'a>(node: &'a AstNode<'a>, out: &mut Vec<Line<'static>>, indent: usize) {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::Heading(h) => render_heading(node, h.level, out),
        NodeValue::Paragraph => render_paragraph(node, out, indent),
        NodeValue::CodeBlock(cb) => render_code_block(cb, out, indent),
        NodeValue::BlockQuote => render_blockquote(node, out),
        NodeValue::List(_) => render_list(node, out, indent),
        NodeValue::Table(_) => render_table(node, out),
        NodeValue::ThematicBreak => out.push(rule_line()),
        NodeValue::HtmlBlock(h) => render_html_block(&h.literal, out),
        NodeValue::Document => {
            for c in node.children() {
                render_block(c, out, indent);
            }
        }
        // Items / rows / cells are handled by their parent list/table.
        NodeValue::Item(_)
        | NodeValue::TaskItem(_)
        | NodeValue::TableRow(_)
        | NodeValue::TableCell
        | NodeValue::FootnoteDefinition(_) => {}
        _ => {}
    }
}

fn rule_line() -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(60),
        Style::default().fg(Color::DarkGray),
    ))
}

fn render_html_block(literal: &str, out: &mut Vec<Line<'static>>) {
    let style = Style::default().fg(Color::DarkGray);
    for line in literal.lines() {
        out.push(Line::from(Span::styled(line.to_string(), style)));
    }
}

fn render_heading<'a>(node: &'a AstNode<'a>, level: u8, out: &mut Vec<Line<'static>>) {
    let (color, prefix) = match level {
        1 => (Color::Cyan, "#"),
        2 => (Color::Blue, "##"),
        3 => (Color::Magenta, "###"),
        4 => (Color::LightMagenta, "####"),
        5 => (Color::Yellow, "#####"),
        _ => (Color::LightYellow, "######"),
    };
    let base = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(
        format!("{prefix} "),
        Style::default().fg(Color::DarkGray),
    ));
    render_inlines(node, &mut spans, base);
    out.push(Line::from(spans));
    if level == 1 {
        out.push(rule_line());
    }
}

fn render_paragraph<'a>(node: &'a AstNode<'a>, out: &mut Vec<Line<'static>>, indent: usize) {
    let indent_str = " ".repeat(indent);
    let mut spans: Vec<Span<'static>> = Vec::new();
    if !indent_str.is_empty() {
        spans.push(Span::raw(indent_str));
    }
    render_inlines(node, &mut spans, Style::default());
    out.push(Line::from(spans));
}

fn render_code_block(cb: &NodeCodeBlock, out: &mut Vec<Line<'static>>, indent: usize) {
    let lang = cb.info.split_whitespace().next().filter(|s| !s.is_empty());
    let prefix = " ".repeat(indent + 2);

    // Mermaid placeholder — M8 will replace with a rendered PNG.
    if matches!(lang, Some("mermaid")) {
        out.push(Line::from(Span::styled(
            format!(
                "╭─ mermaid ({} lines) · install mmdc to render ─",
                cb.literal.lines().count()
            ),
            Style::default().fg(Color::Magenta),
        )));
        for line in cb.literal.lines() {
            out.push(Line::from(Span::styled(
                format!("│ {line}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        out.push(Line::from(Span::styled(
            "╰───────────────────────────────────────────────",
            Style::default().fg(Color::Magenta),
        )));
        return;
    }

    // Opening fence line (dimmed).
    let fence_style = Style::default().fg(Color::DarkGray);
    let mut fence_open = vec![
        Span::styled(prefix.clone(), fence_style),
        Span::styled("```".to_string(), fence_style),
    ];
    if let Some(l) = lang {
        fence_open.push(Span::styled(
            l.to_string(),
            fence_style.add_modifier(Modifier::ITALIC),
        ));
    }
    out.push(Line::from(fence_open));

    // Highlighted body.
    let highlighted = syntax::highlight_code(&cb.literal, lang);
    for highlighted_line in highlighted {
        let mut spans: Vec<Span<'static>> = vec![Span::raw(prefix.clone())];
        spans.extend(highlighted_line.spans);
        out.push(Line::from(spans));
    }

    // Closing fence line.
    out.push(Line::from(vec![
        Span::styled(prefix, fence_style),
        Span::styled("```".to_string(), fence_style),
    ]));
}

fn render_blockquote<'a>(node: &'a AstNode<'a>, out: &mut Vec<Line<'static>>) {
    let mut inner: Vec<Line<'static>> = Vec::new();
    for child in node.children() {
        render_block(child, &mut inner, 0);
    }
    let bar_style = Style::default().fg(Color::DarkGray);
    let text_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::ITALIC);
    for mut line in inner {
        let mut spans = vec![Span::styled("│ ", bar_style)];
        // Restyle every span with the muted italic colour so quote bodies read distinctly.
        for span in line.spans.drain(..) {
            spans.push(Span::styled(span.content.into_owned(), text_style));
        }
        out.push(Line::from(spans));
    }
}

fn render_list<'a>(node: &'a AstNode<'a>, out: &mut Vec<Line<'static>>, indent: usize) {
    let data = node.data.borrow();
    let NodeValue::List(list) = &data.value else {
        return;
    };
    let ordered = matches!(list.list_type, ListType::Ordered);
    let mut idx: u32 = list.start as u32;

    for item in node.children() {
        let item_data = item.data.borrow();
        let prefix = match &item_data.value {
            NodeValue::TaskItem(task) => {
                let checked = task_item_checked(task);
                if checked {
                    format!("{} ☑ ", " ".repeat(indent))
                } else {
                    format!("{} ☐ ", " ".repeat(indent))
                }
            }
            NodeValue::Item(_) if ordered => {
                let p = format!("{}{}. ", " ".repeat(indent), idx);
                idx += 1;
                p
            }
            NodeValue::Item(_) => format!("{}• ", " ".repeat(indent)),
            _ => continue,
        };
        drop(item_data);

        let marker_style = Style::default().fg(Color::Cyan);
        // Render the first block of the item on the same line as the marker.
        let mut children = item.children();
        if let Some(first) = children.next() {
            let mut spans = vec![Span::styled(prefix, marker_style)];
            let first_data = first.data.borrow();
            match &first_data.value {
                NodeValue::Paragraph => {
                    render_inlines(first, &mut spans, Style::default());
                    out.push(Line::from(spans));
                }
                _ => {
                    out.push(Line::from(spans));
                    drop(first_data);
                    render_block(first, out, indent + 2);
                }
            }
        }
        // Any subsequent children (nested lists, extra paragraphs) render indented.
        for c in children {
            render_block(c, out, indent + 2);
        }
    }
}

fn task_item_checked(task: &comrak::nodes::NodeTaskItem) -> bool {
    matches!(task.symbol, Some(c) if c == 'x' || c == 'X')
}

fn render_table<'a>(node: &'a AstNode<'a>, out: &mut Vec<Line<'static>>) {
    // Collect rows of cells as plain strings (styled inline not preserved across
    // the grid for Level A — good-enough; Level B can color bold/italic inside cells).
    let data = node.data.borrow();
    let NodeValue::Table(tbl) = &data.value else {
        return;
    };
    let alignments: Vec<TableAlignment> = tbl.alignments.clone();
    drop(data);

    let mut rows: Vec<Vec<String>> = Vec::new();
    for row_node in node.children() {
        let row_data = row_node.data.borrow();
        let NodeValue::TableRow(_) = &row_data.value else {
            continue;
        };
        drop(row_data);
        let mut row = Vec::new();
        for cell in row_node.children() {
            let cell_data = cell.data.borrow();
            if !matches!(cell_data.value, NodeValue::TableCell) {
                continue;
            }
            drop(cell_data);
            let mut spans: Vec<Span<'static>> = Vec::new();
            render_inlines(cell, &mut spans, Style::default());
            let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
            row.push(text);
        }
        rows.push(row);
    }
    if rows.is_empty() {
        return;
    }
    let ncols = rows[0].len();
    // Compute column widths, cap at 32 cells each to keep narrow panes sane.
    let mut widths = vec![0usize; ncols];
    for row in &rows {
        for (i, cell) in row.iter().enumerate().take(ncols) {
            let w = cell.chars().count().min(32);
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    let border_style = Style::default().fg(Color::DarkGray);
    let header_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    // Top border.
    out.push(Line::from(Span::styled(
        format!(
            "┌{}┐",
            widths
                .iter()
                .map(|w| "─".repeat(w + 2))
                .collect::<Vec<_>>()
                .join("┬")
        ),
        border_style,
    )));

    for (row_idx, row) in rows.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = vec![Span::styled("│", border_style)];
        for (i, cell) in row.iter().enumerate() {
            let width = *widths.get(i).unwrap_or(&0);
            let truncated: String = cell.chars().take(width).collect();
            let pad = width.saturating_sub(truncated.chars().count());
            let aligned = match alignments.get(i).copied().unwrap_or(TableAlignment::None) {
                TableAlignment::Right => format!(" {}{} ", " ".repeat(pad), truncated),
                TableAlignment::Center => {
                    let left = pad / 2;
                    let right = pad - left;
                    format!(" {}{}{} ", " ".repeat(left), truncated, " ".repeat(right))
                }
                _ => format!(" {}{} ", truncated, " ".repeat(pad)),
            };
            let style = if row_idx == 0 {
                header_style
            } else {
                Style::default()
            };
            spans.push(Span::styled(aligned, style));
            spans.push(Span::styled("│", border_style));
        }
        out.push(Line::from(spans));

        // Header separator after first row.
        if row_idx == 0 {
            out.push(Line::from(Span::styled(
                format!(
                    "├{}┤",
                    widths
                        .iter()
                        .map(|w| "─".repeat(w + 2))
                        .collect::<Vec<_>>()
                        .join("┼")
                ),
                border_style,
            )));
        }
    }

    // Bottom border.
    out.push(Line::from(Span::styled(
        format!(
            "└{}┘",
            widths
                .iter()
                .map(|w| "─".repeat(w + 2))
                .collect::<Vec<_>>()
                .join("┴")
        ),
        border_style,
    )));
}

fn render_inlines<'a>(node: &'a AstNode<'a>, spans: &mut Vec<Span<'static>>, base: Style) {
    for child in node.children() {
        let data = child.data.borrow();
        match &data.value {
            NodeValue::Text(t) => spans.push(Span::styled(t.clone(), base)),
            NodeValue::Strong => {
                let s = base.add_modifier(Modifier::BOLD);
                drop(data);
                render_inlines(child, spans, s);
            }
            NodeValue::Emph => {
                let s = base.add_modifier(Modifier::ITALIC);
                drop(data);
                render_inlines(child, spans, s);
            }
            NodeValue::Strikethrough => {
                let s = base.add_modifier(Modifier::CROSSED_OUT);
                drop(data);
                render_inlines(child, spans, s);
            }
            NodeValue::Code(c) => {
                spans.push(Span::styled(
                    c.literal.clone(),
                    Style::default().fg(Color::Cyan).bg(Color::Rgb(30, 30, 40)),
                ));
            }
            NodeValue::Link(_) | NodeValue::WikiLink(_) => {
                let s = base.fg(Color::Blue).add_modifier(Modifier::UNDERLINED);
                drop(data);
                render_inlines(child, spans, s);
            }
            NodeValue::Image(img) => {
                let alt: String = collect_text(child);
                let label = if alt.is_empty() {
                    format!("[image: {}]", img.url)
                } else {
                    format!("[image: {alt}]")
                };
                spans.push(Span::styled(
                    label,
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            NodeValue::HtmlInline(h) => {
                spans.push(Span::styled(
                    h.clone(),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => {
                spans.push(Span::raw(" "));
            }
            NodeValue::FootnoteReference(r) => {
                spans.push(Span::styled(
                    format!("[^{}]", r.name),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            _ => {
                drop(data);
                render_inlines(child, spans, base);
            }
        }
    }
}

pub(crate) fn collect_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut out = String::new();
    for child in node.children() {
        let data = child.data.borrow();
        match &data.value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(c) => out.push_str(&c.literal),
            _ => {
                drop(data);
                out.push_str(&collect_text(child));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_heading_with_rule_after_h1() {
        let lines = render_markdown("# Hello\n\nWorld\n");
        assert!(
            lines.len() >= 3,
            "expected at least heading + rule + paragraph"
        );
        // First line: dim `# ` + bold cyan "Hello"
        let h = &lines[0];
        assert!(h.spans.iter().any(|s| s.content.contains("Hello")));
        // Second line should be the HR under H1.
        assert!(lines[1].spans.iter().any(|s| s.content.contains("─")));
    }

    #[test]
    fn renders_bold_italic_code() {
        let lines = render_markdown("**bold** *italic* `code`\n");
        let paragraph = &lines[0];
        assert!(
            paragraph
                .spans
                .iter()
                .any(|s| s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD)),
            "bold span missing"
        );
        assert!(
            paragraph
                .spans
                .iter()
                .any(|s| s.content == "italic" && s.style.add_modifier.contains(Modifier::ITALIC)),
            "italic span missing"
        );
        assert!(
            paragraph.spans.iter().any(|s| s.content == "code"),
            "code span missing"
        );
    }

    #[test]
    fn renders_a_list_with_bullets() {
        let lines = render_markdown("- one\n- two\n- three\n");
        assert_eq!(lines.len(), 3);
        for line in &lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.contains('•'), "expected bullet, got {text}");
        }
    }

    #[test]
    fn renders_a_task_list() {
        let lines = render_markdown("- [x] done\n- [ ] todo\n");
        assert_eq!(lines.len(), 2);
        let done: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        let todo: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(done.contains('☑'), "checked glyph missing in '{done}'");
        assert!(todo.contains('☐'), "unchecked glyph missing in '{todo}'");
    }

    #[test]
    fn renders_thematic_break() {
        let lines = render_markdown("# a\n\n---\n\n# b\n");
        let rules = lines
            .iter()
            .filter(|l| {
                let t: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                t.chars().all(|c| c == '─')
            })
            .count();
        assert!(rules >= 2, "expected at least an HR + H1 underline");
    }

    #[test]
    fn renders_code_block_with_fences() {
        let lines = render_markdown("```rust\nfn main() {}\n```\n");
        // Expect open fence, at least one code line, close fence.
        assert!(lines.len() >= 3);
        let open: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(open.contains("```"));
        let last: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(last.contains("```"));
    }

    #[test]
    fn renders_mermaid_as_placeholder() {
        let lines = render_markdown("```mermaid\nflowchart LR\nA --> B\n```\n");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("mermaid"), "got:\n{text}");
        assert!(text.contains("mmdc"), "should mention installing mmdc");
    }

    #[test]
    fn renders_a_table() {
        let lines = render_markdown("| A | B |\n|---|---|\n| 1 | 2 |\n");
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains('┌'));
        assert!(joined.contains('│'));
        assert!(joined.contains('└'));
        assert!(joined.contains("A"));
        assert!(joined.contains("1"));
    }

    #[test]
    fn renders_image_as_placeholder() {
        let lines = render_markdown("![a cat](https://example.com/cat.png)\n");
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("[image: a cat]"), "got: {text}");
    }
}
