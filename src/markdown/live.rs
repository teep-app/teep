//! Live-preview block layout.
//!
//! Turns markdown text into a list of `LiveBlock`s, each carrying its
//! source line range and its cooked (rendered) ratatui lines. The live
//! preview widget picks, per block, whether to show the raw source or
//! the cooked form, based on where the cursor currently sits.
//!
//! M7.1: a block whose top-level paragraph is *only* an `![alt](path)`
//! (ignoring whitespace / soft breaks) becomes an image block — its
//! `cooked` lines are blanks reserving vertical space, and the preview
//! overlays a `StatefulImage` widget on that reserved rect.
//!
//! M8: a top-level ```` ```mermaid ```` fence, when `mmdc` is on
//! `$PATH`, becomes either an image block (cache hit) or a
//! `MermaidRef`-tagged placeholder block (cache miss / rendering /
//! failed). The cache-hit path feeds the same M7.1 overlay pipeline.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use comrak::{Arena, nodes::NodeValue, parse_document};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use super::render;

/// Default vertical budget (in terminal rows) reserved for an inline image
/// or mermaid block. Chosen empirically: 12 left wide flowchart diagrams
/// height-constrained to a sliver; 24 lets the typical aspect ratio fill
/// most of the pane width before the height limit kicks in.
pub const INLINE_IMAGE_DEFAULT_ROWS: u16 = 24;

/// Per-block upper bound — a portrait image shouldn't devour the whole
/// preview. User-provided overrides are clamped to this ceiling.
pub const INLINE_IMAGE_MAX_ROWS: u16 = 40;

/// Floor on the reserved height so we never emit a zero-row block — that
/// would collapse the visual anchor entirely and mess with cursor math.
pub const INLINE_IMAGE_MIN_ROWS: u16 = 4;

fn clamp_height(h: u16) -> u16 {
    h.clamp(INLINE_IMAGE_MIN_ROWS, INLINE_IMAGE_MAX_ROWS)
}

/// Rough number of terminal rows a PNG of dims `(w, h)` will occupy when
/// it's aspect-fitted to the preview pane. Assumes a typical ~100-cell
/// pane and terminal cells roughly twice as tall as wide (holds for
/// Ghostty, iTerm2, most monospace fonts). Much narrower or wider panes
/// see a slight mis-reservation, still dramatically better than the
/// fixed default — threading real pane width through the layout
/// pipeline would be a bigger refactor than it's worth for v1.
pub fn image_height_cells(image_w: u32, image_h: u32) -> u16 {
    const ASSUMED_PANE_CELLS: f32 = 100.0;
    const CELL_ASPECT_H_OVER_W: f32 = 2.0;
    if image_w == 0 || image_h == 0 {
        return INLINE_IMAGE_DEFAULT_ROWS;
    }
    let aspect = image_h as f32 / image_w as f32;
    let rows = aspect * ASSUMED_PANE_CELLS / CELL_ASPECT_H_OVER_W;
    clamp_height(rows.round().max(INLINE_IMAGE_MIN_ROWS as f32) as u16)
}

#[derive(Clone, Debug)]
pub struct LiveBlock {
    /// First source line (0-indexed, inclusive).
    pub source_start: usize,
    /// Last source line (0-indexed, exclusive).
    pub source_end: usize,
    /// Pre-cooked ratatui lines for this block. For image blocks this is
    /// `height_cells` blank lines, reserving vertical space that the image
    /// overlay paints over.
    pub cooked: Vec<Line<'static>>,
    /// `Some` when this block is a sole-image paragraph referencing a local
    /// file **or** a mermaid fence whose PNG is already cached. Mixed
    /// "text + image + text" paragraphs stay `None` and fall back to the
    /// inline `[image: alt]` placeholder in `render::render_inlines`.
    pub image: Option<InlineImageRef>,
    /// `Some` when this block is a mermaid fence that needs to be
    /// rendered by `mmdc` (cache miss / currently rendering / failed).
    /// Mutually exclusive with `image`.
    pub mermaid: Option<MermaidRef>,
}

#[derive(Clone, Debug)]
pub struct InlineImageRef {
    /// Absolute path to the image file, already existence-checked.
    pub path: PathBuf,
    pub alt: String,
    /// Rows this block reserves vertically (== `cooked.len()`).
    pub height_cells: u16,
}

#[derive(Clone, Debug)]
pub struct MermaidRef {
    /// The fence body — what `mmdc` consumes as input.
    pub source: String,
    /// blake3 hex of `(source + theme)` — stable cache key.
    pub hash: String,
}

/// Parse `text` and produce one `LiveBlock` per top-level markdown block.
///
/// `markdown_dir`, if provided, is the parent directory of the markdown
/// file being previewed; relative image URLs resolve against it. Pass
/// `None` when the caller has no on-disk anchor (tests, unsaved scratch
/// buffers) — relative image URLs then never materialise into image
/// blocks.
///
/// `mermaid_rendering` is the set of fence hashes currently being
/// rendered by `mmdc`; `mermaid_failed` maps hash → last error message.
/// Both are consulted only to pick the right placeholder text for mermaid
/// blocks that aren't in the cache yet — the scheduling itself happens in
/// the caller.
///
/// Source ranges are 0-indexed line offsets into the raw buffer. Comrak's
/// `sourcepos` is 1-indexed line / column; we translate to 0-indexed lines
/// and treat `end.line` as *inclusive*, so the half-open `[start, end)`
/// we produce spans every buffer line the block occupies.
pub fn parse_blocks(
    text: &str,
    markdown_dir: Option<&Path>,
    mermaid_rendering: &HashSet<String>,
    mermaid_failed: &HashMap<String, String>,
    image_dims: &HashMap<PathBuf, (u32, u32)>,
) -> Vec<LiveBlock> {
    let arena = Arena::new();
    let options = render::build_options();
    let root = parse_document(&arena, text, &options);

    let mut blocks = Vec::new();
    for node in root.children() {
        let data = node.data.borrow();
        let sp = data.sourcepos;
        // comrak: line is 1-indexed; an empty/placeholder block can have
        // end.line < start.line. Clamp defensively.
        let start_line_1 = sp.start.line.max(1);
        let end_line_1 = sp.end.line.max(start_line_1);
        drop(data);

        let source_start = start_line_1 - 1;
        let source_end = end_line_1;

        // Mermaid fences: only dispatched when `mmdc` is reachable — in
        // its absence the node falls through to the default cooked render,
        // which already emits a bordered "install mmdc to render" block.
        let mermaid_fence = detect_mermaid_fence(node);
        let (image, mermaid, cooked) = if let Some((source, height_override)) = mermaid_fence
            && crate::mermaid::is_available()
        {
            let theme = crate::mermaid::DEFAULT_THEME;
            let hash = crate::mermaid::cache_key(&source, theme);
            if let Some(path) = crate::mermaid::cached_path(&hash) {
                // Cache hit — render as a regular image block. Explicit
                // fence override > aspect-aware auto-size > default.
                let auto_height = image_dims
                    .get(&path)
                    .map(|(w, h)| image_height_cells(*w, *h));
                let height = height_override
                    .map(clamp_height)
                    .or(auto_height)
                    .unwrap_or(INLINE_IMAGE_DEFAULT_ROWS);
                let alt = format!("mermaid:{}", &hash[..8.min(hash.len())]);
                let image_ref = InlineImageRef {
                    path,
                    alt,
                    height_cells: height,
                };
                let cooked = vec![Line::from(""); height as usize];
                (Some(image_ref), None, cooked)
            } else {
                // Cache miss — emit a placeholder and tag for scheduling.
                let failed = mermaid_failed.get(&hash).map(|s| s.as_str());
                let cooked =
                    mermaid_placeholder(&source, failed, mermaid_rendering.contains(&hash));
                let mermaid_ref = MermaidRef { source, hash };
                (None, Some(mermaid_ref), cooked)
            }
        } else {
            let image_ref = detect_image_paragraph(node)
                .and_then(|(url, alt)| resolve_local_image(&url, markdown_dir).map(|p| (p, alt)))
                .map(|(path, alt)| {
                    let height_cells = image_dims
                        .get(&path)
                        .map(|(w, h)| image_height_cells(*w, *h))
                        .unwrap_or(INLINE_IMAGE_DEFAULT_ROWS);
                    InlineImageRef {
                        path,
                        alt,
                        height_cells,
                    }
                });
            let cooked = if let Some(r) = &image_ref {
                vec![Line::from(""); r.height_cells as usize]
            } else {
                render::render_block_to_lines(node)
            };
            (image_ref, None, cooked)
        };

        blocks.push(LiveBlock {
            source_start,
            source_end,
            cooked,
            image,
            mermaid,
        });
    }
    blocks
}

/// Cooked placeholder lines for a mermaid fence that's not yet in the
/// cache. Shown while `mmdc` is running or after a failed render — the
/// raw source stays visible inside the border so the user can spot
/// syntax problems without bouncing to source view.
fn mermaid_placeholder(source: &str, failed: Option<&str>, rendering: bool) -> Vec<Line<'static>> {
    let border_style = Style::default().fg(Color::Magenta);
    let dim = Style::default().fg(Color::DarkGray);
    let title = if failed.is_some() {
        "mermaid · failed"
    } else if rendering {
        "mermaid · rendering…"
    } else {
        "mermaid"
    };
    let mut lines = vec![Line::from(Span::styled(
        format!("╭─ {title} ─"),
        border_style,
    ))];
    for line in source.lines() {
        lines.push(Line::from(Span::styled(format!("│ {line}"), dim)));
    }
    if let Some(err) = failed {
        lines.push(Line::from(Span::styled(
            format!("│ ✖ {err}"),
            Style::default().fg(Color::Red),
        )));
    }
    lines.push(Line::from(Span::styled(
        "╰───────────────────────────────────────────────",
        border_style,
    )));
    lines
}

/// If `node` is a top-level mermaid code fence, return its source body
/// and any Teep-specific fence attributes (currently just `height=N`).
///
/// Info-string grammar we recognise:
/// ```text
/// mermaid                  → default height
/// mermaid height=30        → override, clamped to [MIN, MAX] rows
/// mermaid h=30             → same (short form)
/// ```
/// Unknown keys are ignored silently so the fence doesn't break when a
/// later version of Teep adds more attributes.
fn detect_mermaid_fence<'a>(node: &'a comrak::nodes::AstNode<'a>) -> Option<(String, Option<u16>)> {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::CodeBlock(cb) => {
            let mut tokens = cb.info.split_whitespace();
            let lang = tokens.next().unwrap_or("");
            if lang != "mermaid" {
                return None;
            }
            let mut height: Option<u16> = None;
            for tok in tokens {
                if let Some((k, v)) = tok.split_once('=')
                    && matches!(k, "height" | "h")
                    && let Ok(n) = v.parse::<u16>()
                {
                    height = Some(n);
                }
            }
            Some((cb.literal.clone(), height))
        }
        _ => None,
    }
}

/// If `node` is a `Paragraph` whose inline children are exactly one `Image`
/// (ignoring whitespace `Text`, `SoftBreak`, and `LineBreak`), return the
/// image URL and alt text. Otherwise `None` — the caller keeps the default
/// cooked rendering, which emits the `[image: alt]` text placeholder for
/// any inline image embedded in real prose.
fn detect_image_paragraph<'a>(node: &'a comrak::nodes::AstNode<'a>) -> Option<(String, String)> {
    let data = node.data.borrow();
    if !matches!(data.value, NodeValue::Paragraph) {
        return None;
    }
    drop(data);

    let mut image_url: Option<String> = None;
    let mut image_node: Option<&'a comrak::nodes::AstNode<'a>> = None;
    for child in node.children() {
        let cdata = child.data.borrow();
        match &cdata.value {
            NodeValue::Image(img) => {
                if image_url.is_some() {
                    return None; // more than one image → not a sole-image paragraph
                }
                image_url = Some(img.url.clone());
                drop(cdata);
                image_node = Some(child);
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => {}
            NodeValue::Text(t) if t.trim().is_empty() => {}
            _ => return None, // any real content around the image → mixed paragraph
        }
    }
    let url = image_url?;
    let alt = render::collect_text(image_node?);
    Some((url, alt))
}

/// Resolve `url` to a local, existing image file. Returns `None` for URLs
/// that aren't local files (http, https, data, file with scheme),
/// relative paths when `markdown_dir` is absent, or paths that don't exist
/// on disk. Both absolute paths and paths relative to `markdown_dir` are
/// supported.
fn resolve_local_image(url: &str, markdown_dir: Option<&Path>) -> Option<PathBuf> {
    if url.is_empty() {
        return None;
    }
    // Reject anything with a URL scheme we can't locally open.
    if let Some((scheme, _)) = url.split_once(':')
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        && scheme.len() > 1
    {
        // Drive letters on Windows look like "C:..." but this binary is
        // currently macOS/Linux-only; a single-letter "scheme" would have
        // been caught above. Treat any other scheme as non-local.
        return None;
    }

    let candidate = Path::new(url);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        markdown_dir?.join(candidate)
    };

    // Canonicalize if possible (resolves `..` / symlinks); fall back to the
    // joined path when canonicalize fails (e.g. dangling but technically
    // present). `try_exists` is cheap and avoids surfacing permission errors
    // as "missing".
    let resolved = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    if resolved.try_exists().unwrap_or(false) {
        Some(resolved)
    } else {
        None
    }
}

/// Returns the index of the block whose source range contains `row`
/// (0-indexed buffer line), or `None` if `row` falls between blocks
/// (blank lines the parser discarded).
pub fn block_at_row(blocks: &[LiveBlock], row: usize) -> Option<usize> {
    blocks
        .iter()
        .position(|b| row >= b.source_start && row < b.source_end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn two_paragraphs_have_distinct_ranges() {
        let text = "first paragraph\n\nsecond paragraph\n";
        let blocks = parse_blocks(
            text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].source_end <= blocks[1].source_start);
    }

    #[test]
    fn heading_and_code_block_are_separate_blocks() {
        let text = "# Title\n\n```rust\nfn x() {}\n```\n";
        let blocks = parse_blocks(
            text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(blocks.len(), 2, "expected heading + code fence");
        assert_eq!(blocks[0].source_start, 0, "heading starts at line 0");
    }

    #[test]
    fn block_at_row_finds_containing_block() {
        let text = "# Title\n\nBody paragraph.\n";
        let blocks = parse_blocks(
            text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(block_at_row(&blocks, 0), Some(0), "line 0 = heading");
        assert_eq!(block_at_row(&blocks, 2), Some(1), "line 2 = paragraph");
    }

    #[test]
    fn block_at_row_returns_none_for_blank_row_between_blocks() {
        let text = "one\n\nthree\n";
        let blocks = parse_blocks(
            text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        // Line 1 is the blank between two paragraphs — not owned by either.
        assert_eq!(block_at_row(&blocks, 1), None);
    }

    #[test]
    fn cooked_lines_are_populated() {
        let blocks = parse_blocks(
            "# Hello\n",
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(!blocks[0].cooked.is_empty());
    }

    #[test]
    fn image_height_cells_wide_flowchart() {
        // A 4:1 mermaid PNG should aspect-fit to ~13 rows at our 100-cell
        // assumption, comfortably under the 40-row ceiling.
        let h = image_height_cells(2400, 600);
        assert!(
            (12..=14).contains(&h),
            "expected ~13 rows for 4:1 flowchart, got {h}"
        );
    }

    #[test]
    fn image_height_cells_portrait_clamps() {
        // A portrait photo would compute tall enough to eat the pane; cap
        // enforces the max.
        assert_eq!(image_height_cells(800, 1600), INLINE_IMAGE_MAX_ROWS);
    }

    #[test]
    fn image_height_cells_zero_dims_returns_default() {
        assert_eq!(image_height_cells(0, 0), INLINE_IMAGE_DEFAULT_ROWS);
        assert_eq!(image_height_cells(100, 0), INLINE_IMAGE_DEFAULT_ROWS);
        assert_eq!(image_height_cells(0, 100), INLINE_IMAGE_DEFAULT_ROWS);
    }

    #[test]
    fn parse_blocks_uses_aspect_aware_height_when_dims_known() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let png = write_tmp_png(tmp.path(), "diagram.png");
        // `resolve_local_image` canonicalizes; key the dims map on the
        // same canonical form or the lookup misses (on macOS /tmp is a
        // symlink to /private/tmp).
        let canonical = std::fs::canonicalize(&png).expect("canonicalize");
        let text = format!("![]({})\n", png.display());

        // Without dims: default height.
        let blocks = parse_blocks(
            &text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            blocks[0].image.as_ref().unwrap().height_cells,
            INLINE_IMAGE_DEFAULT_ROWS,
        );

        // With dims (wide, 4:1): aspect-aware height.
        let mut dims = HashMap::new();
        dims.insert(canonical, (2400u32, 600u32));
        let blocks = parse_blocks(&text, None, &HashSet::new(), &HashMap::new(), &dims);
        let h = blocks[0].image.as_ref().unwrap().height_cells;
        assert!(
            h < INLINE_IMAGE_DEFAULT_ROWS,
            "wide image should reserve fewer rows than default, got {h}",
        );
        assert_eq!(blocks[0].cooked.len(), h as usize);
    }

    fn write_tmp_png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        // Minimal 1x1 PNG (produced out-of-band, stored as bytes); we only
        // need the file to exist for path resolution, not to decode.
        let mut f = std::fs::File::create(&path).expect("create tmp png");
        f.write_all(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
            .expect("write png header");
        path
    }

    #[test]
    fn sole_image_paragraph_becomes_image_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_tmp_png(tmp.path(), "a.png");

        let text = "![alt text](a.png)\n";
        let blocks = parse_blocks(
            text,
            Some(tmp.path()),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(blocks.len(), 1);
        let img = blocks[0].image.as_ref().expect("image block");
        assert_eq!(img.alt, "alt text");
        assert!(img.path.ends_with("a.png"));
        assert_eq!(blocks[0].cooked.len(), INLINE_IMAGE_DEFAULT_ROWS as usize);
    }

    #[test]
    fn image_mixed_with_text_stays_text_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_tmp_png(tmp.path(), "b.png");

        let text = "look: ![x](b.png) inline\n";
        let blocks = parse_blocks(
            text,
            Some(tmp.path()),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(blocks.len(), 1);
        assert!(
            blocks[0].image.is_none(),
            "mixed paragraph must not be an image block"
        );
    }

    #[test]
    fn missing_image_falls_back_to_text_block() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let text = "![](nope.png)\n";
        let blocks = parse_blocks(
            text,
            Some(tmp.path()),
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(blocks[0].image.is_none());
    }

    #[test]
    fn http_image_falls_back_to_text_block() {
        let text = "![](https://example.com/x.png)\n";
        let blocks = parse_blocks(
            text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(blocks[0].image.is_none());
    }

    #[test]
    fn data_uri_image_falls_back_to_text_block() {
        let text = "![](data:image/png;base64,AAAA)\n";
        let blocks = parse_blocks(
            text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(blocks[0].image.is_none());
    }

    #[test]
    fn relative_image_without_markdown_dir_falls_back() {
        // No markdown_dir → relative path cannot resolve.
        let text = "![](a.png)\n";
        let blocks = parse_blocks(
            text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(blocks[0].image.is_none());
    }

    #[test]
    fn absolute_image_path_resolves() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = write_tmp_png(tmp.path(), "abs.png");
        let text = format!("![]({})\n", p.display());
        let blocks = parse_blocks(
            &text,
            None,
            &HashSet::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(blocks[0].image.is_some(), "absolute path should resolve");
    }

    /// Helper to parse a mermaid fence info string via a synthetic
    /// `graph TD` body, bypassing the `mmdc` presence check. We call
    /// `detect_mermaid_fence` directly to get at the height override.
    fn detect_fence_from_text(text: &str) -> Option<(String, Option<u16>)> {
        let arena = Arena::new();
        let options = render::build_options();
        let root = parse_document(&arena, text, &options);
        root.children().next().and_then(detect_mermaid_fence)
    }

    #[test]
    fn mermaid_info_string_without_attrs_has_no_override() {
        let got = detect_fence_from_text("```mermaid\ngraph TD\nA-->B\n```\n");
        assert_eq!(got.as_ref().map(|(_, h)| *h), Some(None));
    }

    #[test]
    fn mermaid_info_string_height_attr_parses() {
        let got = detect_fence_from_text("```mermaid height=30\ngraph TD\nA-->B\n```\n");
        assert_eq!(got.as_ref().and_then(|(_, h)| *h), Some(30));
    }

    #[test]
    fn mermaid_info_string_h_short_form_parses() {
        let got = detect_fence_from_text("```mermaid h=18\ngraph TD\nA-->B\n```\n");
        assert_eq!(got.as_ref().and_then(|(_, h)| *h), Some(18));
    }

    #[test]
    fn mermaid_info_string_unknown_attrs_ignored() {
        let got = detect_fence_from_text("```mermaid theme=forest height=20 mystery=?\n\n```\n");
        assert_eq!(got.as_ref().and_then(|(_, h)| *h), Some(20));
    }

    #[test]
    fn clamp_height_bounds() {
        assert_eq!(clamp_height(0), INLINE_IMAGE_MIN_ROWS);
        assert_eq!(clamp_height(999), INLINE_IMAGE_MAX_ROWS);
        assert_eq!(clamp_height(20), 20);
    }
}
