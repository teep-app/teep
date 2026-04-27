//! Span-aware soft word wrap for cooked markdown lines.
//!
//! `ratatui::widgets::Paragraph` has a `Wrap` mode, but we can't use it
//! inside the live-preview render path: cursor visibility math there
//! assumes one terminal row per `Line` we push, and Paragraph's wrap
//! would silently expand some lines onto extra rows behind our back.
//! This helper does the wrap up front, so the row count we hand to
//! Paragraph matches what gets drawn — and so image-overlay rects
//! (`render_live_preview` second pass) still land on the right rows.
//!
//! Greedy whitespace wrap; preserves each character's `Style`; coalesces
//! consecutive same-style chars back into spans on each output line.

use ratatui::{
    style::Style,
    text::{Line, Span},
};

/// Wrap `line` to fit `width` terminal cells.
///
/// Returns the input unchanged when `width == 0`, the line is empty, or
/// the line already fits. Hard-breaks long unbroken runs (URLs, code
/// fragments) at the width boundary so we never emit a line wider than
/// `width`.
pub fn wrap_styled_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 || line.spans.is_empty() {
        return vec![line.clone()];
    }
    let total_chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if total_chars <= width {
        return vec![line.clone()];
    }

    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| {
            let style = s.style;
            s.content.chars().map(move |c| (c, style))
        })
        .collect();

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut line_start = 0usize;
    let mut last_ws_end = 0usize; // char index just past the most recent whitespace
    let mut i = 0usize;
    while i < chars.len() {
        if i - line_start >= width {
            let break_at = if last_ws_end > line_start {
                last_ws_end
            } else {
                i // no whitespace in this run — hard break
            };
            let mut piece_end = break_at;
            while piece_end > line_start && chars[piece_end - 1].0.is_whitespace() {
                piece_end -= 1;
            }
            out.push(build_line(&chars[line_start..piece_end]));
            line_start = break_at;
            while line_start < chars.len() && chars[line_start].0.is_whitespace() {
                line_start += 1;
            }
            last_ws_end = line_start;
            i = line_start;
            continue;
        }
        if chars[i].0.is_whitespace() {
            last_ws_end = i + 1;
        }
        i += 1;
    }
    if line_start < chars.len() {
        out.push(build_line(&chars[line_start..]));
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

fn build_line(slice: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut k = 0;
    while k < slice.len() {
        let style = slice[k].1;
        let mut j = k + 1;
        while j < slice.len() && slice[j].1 == style {
            j += 1;
        }
        let text: String = slice[k..j].iter().map(|(c, _)| *c).collect();
        spans.push(Span::styled(text, style));
        k = j;
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn plain(text: &str) -> Line<'static> {
        Line::from(Span::raw(text.to_string()))
    }

    #[test]
    fn short_line_passes_through() {
        let line = plain("hello world");
        let out = wrap_styled_line(&line, 80);
        assert_eq!(out.len(), 1);
        let joined: String = out[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "hello world");
    }

    #[test]
    fn zero_width_returns_input() {
        let line = plain("anything goes");
        let out = wrap_styled_line(&line, 0);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn empty_line_returns_input() {
        let line = Line::from("");
        let out = wrap_styled_line(&line, 10);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn wraps_on_whitespace() {
        let line = plain("the quick brown fox jumps over the lazy dog");
        let out = wrap_styled_line(&line, 20);
        assert!(out.len() >= 2);
        for l in &out {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                text.chars().count() <= 20,
                "wrapped line too wide: {:?} ({} chars)",
                text,
                text.chars().count()
            );
        }
        // No content lost.
        let joined: String = out
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("the"));
        assert!(joined.contains("dog"));
    }

    #[test]
    fn hard_breaks_unbroken_run() {
        // No whitespace anywhere — must hard-break at width.
        let line = plain("aaaaaaaaaaaaaaaaaaaaaaaaaa"); // 26 chars
        let out = wrap_styled_line(&line, 10);
        assert!(out.len() >= 3);
        for l in &out {
            let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.chars().count() <= 10);
        }
    }

    #[test]
    fn preserves_styles_across_wrap() {
        // Plain "lorem " then bold "ipsum dolor sit amet consectetur".
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let line = Line::from(vec![
            Span::raw("lorem ".to_string()),
            Span::styled("ipsum dolor sit amet consectetur".to_string(), bold),
        ]);
        let out = wrap_styled_line(&line, 14);
        assert!(out.len() >= 2);
        // At least one span on a wrapped line keeps the bold modifier.
        let any_bold = out
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(any_bold, "bold style was lost during wrap");
    }

    #[test]
    fn preserves_color_and_does_not_smear_into_neighbours() {
        let red = Style::default().fg(Color::Red);
        let line = Line::from(vec![
            Span::raw("plain ".to_string()),
            Span::styled("redred ".to_string(), red),
            Span::raw("tail end of the paragraph here".to_string()),
        ]);
        let out = wrap_styled_line(&line, 10);
        // Red text should still be tagged red, plain text should not be red.
        for l in &out {
            for s in &l.spans {
                if s.content.contains("red") {
                    assert_eq!(s.style.fg, Some(Color::Red));
                }
                if s.content.contains("plain") || s.content.contains("tail") {
                    assert_ne!(s.style.fg, Some(Color::Red));
                }
            }
        }
    }
}
