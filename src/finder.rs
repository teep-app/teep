use std::{path::PathBuf, sync::Arc};

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::tree::{Node, NodeKind};

/// Max matches we keep per refresh. Ratatui rendering and human cognition
/// both cap out well before this.
const MAX_MATCHES: usize = 200;

#[derive(Clone, Debug)]
pub struct FinderItem {
    pub path: PathBuf,
    /// What gets searched and rendered — usually the path relative to repo root.
    pub display: String,
}

#[derive(Clone, Copy, Debug)]
pub struct MatchedItem {
    pub index: usize,
    #[allow(dead_code)] // reserved for potential relevance tie-breakers / UI weights
    pub score: u32,
}

pub struct FinderState {
    pub query: String,
    pub items: Arc<Vec<FinderItem>>,
    pub matches: Vec<MatchedItem>,
    pub selected: usize,
    matcher: Matcher,
}

impl FinderState {
    pub fn new(items: Vec<FinderItem>) -> Self {
        let mut s = Self {
            query: String::new(),
            items: Arc::new(items),
            matches: Vec::new(),
            selected: 0,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        };
        s.refresh();
        s
    }

    pub fn push(&mut self, c: char) {
        self.query.push(c);
        self.refresh();
    }

    pub fn pop(&mut self) {
        if self.query.pop().is_some() {
            self.refresh();
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.matches.len() {
            self.selected += 1;
        }
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn selected_path(&self) -> Option<PathBuf> {
        let m = self.matches.get(self.selected)?;
        self.items.get(m.index).map(|i| i.path.clone())
    }

    fn refresh(&mut self) {
        self.selected = 0;
        self.matches.clear();

        if self.query.is_empty() {
            for (i, _) in self.items.iter().enumerate().take(MAX_MATCHES) {
                self.matches.push(MatchedItem { index: i, score: 0 });
            }
            return;
        }

        let pattern = Pattern::parse(&self.query, CaseMatching::Smart, Normalization::Smart);
        let mut scored: Vec<(u32, usize)> = Vec::with_capacity(self.items.len().min(MAX_MATCHES));
        let mut buf: Vec<char> = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            buf.clear();
            let hay = Utf32Str::new(&item.display, &mut buf);
            if let Some(score) = pattern.score(hay, &mut self.matcher) {
                scored.push((score, i));
            }
        }
        // Descending by score; path-like haystacks tend to have well-separated scores.
        scored.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        for (score, i) in scored.into_iter().take(MAX_MATCHES) {
            self.matches.push(MatchedItem { index: i, score });
        }
    }
}

/// Flatten a tree into a list of file-only items, with display paths relative
/// to the tree's root where possible.
pub fn items_from_tree(root: &Node) -> Vec<FinderItem> {
    let mut out = Vec::new();
    collect(root, &root.path, &mut out);
    out
}

fn collect(node: &Node, root: &std::path::Path, out: &mut Vec<FinderItem>) {
    if matches!(node.kind, NodeKind::File) {
        let display = node
            .path
            .strip_prefix(root)
            .unwrap_or(node.path.as_path())
            .display()
            .to_string();
        out.push(FinderItem {
            path: node.path.clone(),
            display,
        });
    }
    for c in &node.children {
        collect(c, root, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(s: &str) -> FinderItem {
        FinderItem {
            path: PathBuf::from(s),
            display: s.to_string(),
        }
    }

    #[test]
    fn empty_query_shows_all_capped() {
        let items = (0..500).map(|i| item(&format!("file_{i}.rs"))).collect();
        let f = FinderState::new(items);
        assert_eq!(f.matches.len(), MAX_MATCHES);
    }

    #[test]
    fn typing_narrows_matches() {
        let items = vec![
            item("src/main.rs"),
            item("src/lib.rs"),
            item("README.md"),
            item("tests/integration.rs"),
        ];
        let mut f = FinderState::new(items);
        for c in "main".chars() {
            f.push(c);
        }
        assert!(
            f.matches
                .iter()
                .map(|m| f.items[m.index].display.as_str())
                .any(|s| s == "src/main.rs"),
            "main.rs should appear in matches"
        );
        // Top result should contain 'main'.
        let top = &f.items[f.matches[0].index].display;
        assert!(
            top.contains("main"),
            "top match should match query, got {top}"
        );
    }

    #[test]
    fn selected_path_returns_current_match() {
        let items = vec![item("a.rs"), item("b.rs")];
        let f = FinderState::new(items);
        let p = f.selected_path().unwrap();
        let s = p.to_str().unwrap();
        assert!(s == "a.rs" || s == "b.rs");
    }
}
