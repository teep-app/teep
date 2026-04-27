//! Markdown rendering (Level A — source-with-styling / beautified preview).
//!
//! Parses GFM via `comrak` and emits a flat `Vec<Line<'static>>` styled for
//! ratatui. The preview widget renders these lines in place of the viewer's
//! syntect-highlighted source when the user toggles `m`.
//!
//! Intentionally omitted in Level A:
//! - Inline images (M7 replaces `[image]` placeholders with Kitty-protocol pictures).
//! - Mermaid diagrams (M8 replaces `[mermaid]` placeholders with rendered PNGs).
//! - Reveal-on-cursor editing (M6.5 / Level B).

pub mod live;
pub mod render;
pub mod wrap;

pub use live::{InlineImageRef, LiveBlock, block_at_row, parse_blocks};
pub use wrap::wrap_styled_line;
