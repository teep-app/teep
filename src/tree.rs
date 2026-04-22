use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use ignore::WalkBuilder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Dir,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub kind: NodeKind,
    pub expanded: bool,
    pub children: Vec<Node>,
}

pub struct Tree {
    pub root: Node,
    pub selected: PathBuf,
    pub scroll: usize,
}

impl Tree {
    /// Walk `root` (respecting .gitignore) and build an initial tree.
    /// Top-level is pre-expanded; subdirectories start collapsed to keep
    /// the sidebar manageable on large repos.
    pub fn build(root: &Path) -> Result<Self> {
        let mut node = build_node(root)?;
        if let NodeKind::Dir = node.kind {
            node.expanded = true;
        }
        let selected = node.path.clone();
        let mut tree = Self {
            root: node,
            selected,
            scroll: 0,
        };
        tree.select_first();
        Ok(tree)
    }

    /// Replace the tree's root with a freshly-walked `new_root`, preserving
    /// which directories are expanded and the user's selection when possible.
    pub fn graft(&mut self, mut new_root: Node) {
        let expanded = collect_expanded_paths(&self.root);
        apply_expanded(&mut new_root, &expanded);
        if matches!(new_root.kind, NodeKind::Dir) {
            new_root.expanded = true;
        }
        self.root = new_root;
        if self.selected_node().is_none() {
            self.select_first();
        }
    }

    #[cfg(test)]
    pub fn for_testing(root_path: std::path::PathBuf) -> Self {
        Self {
            root: Node {
                name: "test".to_string(),
                path: root_path.clone(),
                kind: NodeKind::Dir,
                expanded: true,
                children: Vec::new(),
            },
            selected: root_path,
            scroll: 0,
        }
    }

    /// All currently-visible nodes as `(depth, node)`, skipping the root row.
    pub fn visible(&self) -> Vec<(usize, &Node)> {
        let mut out = Vec::new();
        collect_visible(&self.root, 0, &mut out);
        // Skip the root itself; callers render children of the root.
        out.into_iter().skip(1).collect()
    }

    pub fn selected_node(&self) -> Option<&Node> {
        find_node(&self.root, &self.selected)
    }

    pub fn select_first(&mut self) {
        if let Some((_, n)) = self.visible().first() {
            self.selected = n.path.clone();
        }
    }

    pub fn move_down(&mut self) {
        let visible = self.visible();
        let Some(idx) = visible.iter().position(|(_, n)| n.path == self.selected) else {
            if let Some((_, n)) = visible.first() {
                self.selected = n.path.clone();
            }
            return;
        };
        if let Some((_, n)) = visible.get(idx + 1) {
            self.selected = n.path.clone();
        }
    }

    pub fn move_up(&mut self) {
        let visible = self.visible();
        let Some(idx) = visible.iter().position(|(_, n)| n.path == self.selected) else {
            if let Some((_, n)) = visible.first() {
                self.selected = n.path.clone();
            }
            return;
        };
        if idx > 0 {
            self.selected = visible[idx - 1].1.path.clone();
        }
    }

    /// Expand the selected dir, or no-op for files.
    pub fn expand_selected(&mut self) {
        let path = self.selected.clone();
        set_expanded(&mut self.root, &path, true);
    }

    pub fn collapse_selected(&mut self) {
        let path = self.selected.clone();
        set_expanded(&mut self.root, &path, false);
    }

    pub fn toggle_selected(&mut self) {
        let path = self.selected.clone();
        toggle_expanded(&mut self.root, &path);
    }
}

fn collect_visible<'a>(node: &'a Node, depth: usize, out: &mut Vec<(usize, &'a Node)>) {
    out.push((depth, node));
    if matches!(node.kind, NodeKind::Dir) && node.expanded {
        for c in &node.children {
            collect_visible(c, depth + 1, out);
        }
    }
}

fn find_node<'a>(node: &'a Node, path: &Path) -> Option<&'a Node> {
    if node.path == path {
        return Some(node);
    }
    for c in &node.children {
        if let Some(n) = find_node(c, path) {
            return Some(n);
        }
    }
    None
}

fn set_expanded(node: &mut Node, path: &Path, expanded: bool) {
    if node.path == path {
        if matches!(node.kind, NodeKind::Dir) {
            node.expanded = expanded;
        }
        return;
    }
    for c in &mut node.children {
        set_expanded(c, path, expanded);
    }
}

fn toggle_expanded(node: &mut Node, path: &Path) {
    if node.path == path {
        if matches!(node.kind, NodeKind::Dir) {
            node.expanded = !node.expanded;
        }
        return;
    }
    for c in &mut node.children {
        toggle_expanded(c, path);
    }
}

/// Recursively walk `root` via `ignore::WalkBuilder` and materialize a `Node` tree.
/// `WalkBuilder` yields a flat depth-first list; we rebuild the hierarchy from paths.
/// Public so the runtime can rebuild the tree on a blocking thread.
pub fn build_node(root: &Path) -> Result<Node> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", root.display()))?;
    // Gather paths with their depths.
    let mut paths: BTreeMap<PathBuf, bool> = BTreeMap::new(); // true = is_dir
    let walker = WalkBuilder::new(&root)
        .follow_links(false)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .build();
    for entry in walker.flatten() {
        let p = entry.path().to_path_buf();
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        paths.insert(p, is_dir);
    }

    // Build tree from sorted paths.
    let root_is_dir = paths.get(&root).copied().unwrap_or(true);
    let mut root_node = Node {
        name: root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(".")
            .to_string(),
        path: root.clone(),
        kind: if root_is_dir {
            NodeKind::Dir
        } else {
            NodeKind::File
        },
        expanded: false,
        children: Vec::new(),
    };
    for (p, is_dir) in paths {
        if p == root {
            continue;
        }
        insert(&mut root_node, &p, is_dir);
    }
    sort_tree(&mut root_node);
    Ok(root_node)
}

fn collect_expanded_paths(node: &Node) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    walk_expanded(node, &mut out);
    out
}

fn walk_expanded(node: &Node, out: &mut HashSet<PathBuf>) {
    if matches!(node.kind, NodeKind::Dir) && node.expanded {
        out.insert(node.path.clone());
    }
    for c in &node.children {
        walk_expanded(c, out);
    }
}

fn apply_expanded(node: &mut Node, expanded: &HashSet<PathBuf>) {
    if matches!(node.kind, NodeKind::Dir) && expanded.contains(&node.path) {
        node.expanded = true;
    }
    for c in &mut node.children {
        apply_expanded(c, expanded);
    }
}

fn insert(parent: &mut Node, path: &Path, is_dir: bool) {
    let relative = match path.strip_prefix(&parent.path) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut components = relative.components();
    let Some(first) = components.next() else {
        return;
    };
    let name = first.as_os_str().to_string_lossy().into_owned();
    let child_path = parent.path.join(&name);
    let rest_empty = components.next().is_none();

    let child_idx = parent
        .children
        .iter()
        .position(|c| c.name == name)
        .unwrap_or_else(|| {
            let new_node = Node {
                name: name.clone(),
                path: child_path.clone(),
                kind: if rest_empty && !is_dir {
                    NodeKind::File
                } else {
                    NodeKind::Dir
                },
                expanded: false,
                children: Vec::new(),
            };
            parent.children.push(new_node);
            parent.children.len() - 1
        });

    if !rest_empty {
        insert(&mut parent.children[child_idx], path, is_dir);
    } else if is_dir {
        parent.children[child_idx].kind = NodeKind::Dir;
    }
}

fn sort_tree(node: &mut Node) {
    node.children.sort_by(|a, b| match (a.kind, b.kind) {
        (NodeKind::Dir, NodeKind::File) => std::cmp::Ordering::Less,
        (NodeKind::File, NodeKind::Dir) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    for c in &mut node.children {
        sort_tree(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, children: Vec<Node>) -> Node {
        Node {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind: NodeKind::Dir,
            expanded: true,
            children,
        }
    }

    fn file(name: &str) -> Node {
        Node {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind: NodeKind::File,
            expanded: false,
            children: vec![],
        }
    }

    fn collapsed_dir(name: &str, children: Vec<Node>) -> Node {
        let mut d = dir(name, children);
        d.expanded = false;
        d
    }

    #[test]
    fn visible_skips_root_and_collapsed_subtrees() {
        let t = Tree {
            root: dir(
                "/",
                vec![
                    dir("src", vec![file("main.rs"), file("lib.rs")]),
                    file("Cargo.toml"),
                ],
            ),
            selected: PathBuf::from("/"),
            scroll: 0,
        };
        let visible: Vec<&str> = t.visible().iter().map(|(_, n)| n.name.as_str()).collect();
        assert_eq!(visible, vec!["src", "main.rs", "lib.rs", "Cargo.toml"]);
    }

    #[test]
    fn graft_preserves_expand_state() {
        let old_root = dir("/r", vec![dir("/r/src", vec![file("/r/src/main.rs")])]);
        let mut t = Tree {
            root: old_root,
            selected: PathBuf::from("/r/src/main.rs"),
            scroll: 0,
        };
        // New walk: same shape, src initially collapsed (as fresh walks always are).
        let new_root = collapsed_dir(
            "/r",
            vec![collapsed_dir(
                "/r/src",
                vec![file("/r/src/main.rs"), file("/r/src/new.rs")],
            )],
        );
        t.graft(new_root);
        // Root is always expanded post-graft.
        assert!(t.root.expanded);
        // src was expanded before, should still be expanded.
        let src = &t.root.children[0];
        assert!(src.expanded, "graft should preserve expanded src");
        // Selection preserved.
        assert_eq!(t.selected, PathBuf::from("/r/src/main.rs"));
    }

    #[test]
    fn graft_reselects_when_selection_disappears() {
        let old_root = dir("/r", vec![dir("/r/src", vec![file("/r/src/gone.rs")])]);
        let mut t = Tree {
            root: old_root,
            selected: PathBuf::from("/r/src/gone.rs"),
            scroll: 0,
        };
        let new_root = dir("/r", vec![dir("/r/src", vec![file("/r/src/other.rs")])]);
        t.graft(new_root);
        // Selection must now point to something that exists in the tree.
        assert!(t.selected_node().is_some());
    }
}
