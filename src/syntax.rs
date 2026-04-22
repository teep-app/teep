use std::{path::Path, sync::OnceLock};

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Style as SynStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        let set = ThemeSet::load_defaults();
        set.themes
            .get("base16-ocean.dark")
            .cloned()
            .or_else(|| set.themes.values().next().cloned())
            .expect("syntect ships at least one default theme")
    })
}

/// Highlight a full file into ratatui `Line`s. Suitable for moderately-sized
/// files (<~2 MB); callers should guard on size and fall back to plain text.
pub fn highlight_file(text: &str, path: &Path) -> Vec<Line<'static>> {
    let ss = syntax_set();
    let syntax = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| ss.find_syntax_by_extension(ext))
        .or_else(|| {
            path.file_name()
                .and_then(|f| f.to_str())
                .and_then(|n| ss.find_syntax_by_token(n))
        })
        .unwrap_or_else(|| ss.find_syntax_plain_text());

    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut out = Vec::new();
    for line in LinesWithEndings::from(text) {
        let ranges = highlighter.highlight_line(line, ss).unwrap_or_default();
        out.push(ranges_to_line(&ranges));
    }
    out
}

fn ranges_to_line(ranges: &[(SynStyle, &str)]) -> Line<'static> {
    let spans: Vec<Span<'static>> = ranges
        .iter()
        .filter_map(|(style, text)| {
            // Drop trailing newline — ratatui Line wraps itself.
            let text = text.trim_end_matches('\n');
            if text.is_empty() {
                return None;
            }
            let fg = style.foreground;
            let mut s = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));
            if style.font_style.contains(FontStyle::BOLD) {
                s = s.add_modifier(ratatui::style::Modifier::BOLD);
            }
            if style.font_style.contains(FontStyle::ITALIC) {
                s = s.add_modifier(ratatui::style::Modifier::ITALIC);
            }
            if style.font_style.contains(FontStyle::UNDERLINE) {
                s = s.add_modifier(ratatui::style::Modifier::UNDERLINED);
            }
            Some(Span::styled(text.to_string(), s))
        })
        .collect();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_a_rust_file() {
        let lines = highlight_file("fn main() { let x = 1; }\n", Path::new("x.rs"));
        assert!(!lines.is_empty());
    }

    #[test]
    fn falls_back_to_plain_text_for_unknown_ext() {
        let lines = highlight_file("just some text\n", Path::new("notes.qqq"));
        assert_eq!(lines.len(), 1);
    }
}
