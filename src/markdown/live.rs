//! Live-preview block layout.
//!
//! Turns markdown text into a list of `LiveBlock`s, each carrying its
//! source line range and its cooked (rendered) ratatui lines. The live
//! preview widget picks, per block, whether to show the raw source or
//! the cooked form, based on where the cursor currently sits.

use comrak::{Arena, parse_document};
use ratatui::text::Line;

use super::render;

#[derive(Clone, Debug)]
pub struct LiveBlock {
    /// First source line (0-indexed, inclusive).
    pub source_start: usize,
    /// Last source line (0-indexed, exclusive).
    pub source_end: usize,
    /// Pre-cooked ratatui lines for this block.
    pub cooked: Vec<Line<'static>>,
}

/// Parse `text` and produce one `LiveBlock` per top-level markdown block.
///
/// Source ranges are 0-indexed line offsets into the raw buffer. Comrak's
/// `sourcepos` is 1-indexed line / column; we translate to 0-indexed lines
/// and treat `end.line` as *inclusive*, so the half-open `[start, end)`
/// we produce spans every buffer line the block occupies.
pub fn parse_blocks(text: &str) -> Vec<LiveBlock> {
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

        let cooked = render::render_block_to_lines(node);

        blocks.push(LiveBlock {
            source_start,
            source_end,
            cooked,
        });
    }
    blocks
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

    #[test]
    fn two_paragraphs_have_distinct_ranges() {
        let text = "first paragraph\n\nsecond paragraph\n";
        let blocks = parse_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].source_end <= blocks[1].source_start);
    }

    #[test]
    fn heading_and_code_block_are_separate_blocks() {
        let text = "# Title\n\n```rust\nfn x() {}\n```\n";
        let blocks = parse_blocks(text);
        assert_eq!(blocks.len(), 2, "expected heading + code fence");
        assert_eq!(blocks[0].source_start, 0, "heading starts at line 0");
    }

    #[test]
    fn block_at_row_finds_containing_block() {
        let text = "# Title\n\nBody paragraph.\n";
        let blocks = parse_blocks(text);
        assert_eq!(block_at_row(&blocks, 0), Some(0), "line 0 = heading");
        assert_eq!(block_at_row(&blocks, 2), Some(1), "line 2 = paragraph");
    }

    #[test]
    fn block_at_row_returns_none_for_blank_row_between_blocks() {
        let text = "one\n\nthree\n";
        let blocks = parse_blocks(text);
        // Line 1 is the blank between two paragraphs — not owned by either.
        assert_eq!(block_at_row(&blocks, 1), None);
    }

    #[test]
    fn cooked_lines_are_populated() {
        let blocks = parse_blocks("# Hello\n");
        assert!(!blocks[0].cooked.is_empty());
    }
}
