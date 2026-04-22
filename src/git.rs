use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow};
use similar::{ChangeTag, TextDiff};

/// Snapshot of read-only git state we surface in HITLed.
///
/// gix is used only to verify that a directory is a git repo at all
/// (it surfaces the most useful error when it's not). Everything else
/// shells out to the `git` CLI: the output formats are stable and
/// documented, and this avoids wrestling with gix's extensive but
/// churny API for data we only read. Revisiting as an M9 optimization.
#[derive(Clone, Debug, Default)]
pub struct GitSnapshot {
    pub branch: Option<String>,
    pub head_short: Option<String>,
    pub worktree_path: PathBuf,
    pub worktrees: Vec<WorktreeEntry>,
    pub branches: Vec<String>,
    pub status: Vec<StatusEntry>,
    pub is_clean: bool,
}

#[derive(Clone, Debug)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_current: bool,
}

#[derive(Clone, Debug)]
pub struct StatusEntry {
    pub path: PathBuf,
    pub kind: StatusKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Ignored,
    Conflicted,
}

impl StatusKind {
    pub fn glyph(&self) -> &'static str {
        match self {
            StatusKind::Modified => "M",
            StatusKind::Added => "A",
            StatusKind::Deleted => "D",
            StatusKind::Renamed => "R",
            StatusKind::Untracked => "?",
            StatusKind::Ignored => "!",
            StatusKind::Conflicted => "C",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
    // Retained for a future "line numbers in diff gutter" polish pass; not rendered today.
    #[allow(dead_code)]
    pub old_lineno: Option<u32>,
    #[allow(dead_code)]
    pub new_lineno: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    HunkHeader,
}

/// Build a snapshot. Suitable for `spawn_blocking` — runs several git
/// commands serially, typically under 100 ms on small/medium repos.
pub fn snapshot(root: &Path) -> Result<GitSnapshot> {
    // Verify this is actually a git repo before spending time on it.
    gix::discover(root).map_err(|e| anyhow!("not a git repository at {}: {e}", root.display()))?;

    let worktree_path = resolve_worktree_root(root).unwrap_or_else(|_| root.to_path_buf());
    let branch = current_branch(root).ok();
    let head_short = head_short(root).ok();
    let worktrees = list_worktrees(root, &worktree_path).unwrap_or_default();
    let branches = list_local_branches(root).unwrap_or_default();
    let status = porcelain_status(root, &worktree_path).unwrap_or_default();
    let is_clean = status.is_empty();

    Ok(GitSnapshot {
        branch,
        head_short,
        worktree_path,
        worktrees,
        branches,
        status,
        is_clean,
    })
}

fn git_at(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root);
    c
}

fn run(mut cmd: Command) -> Result<String> {
    let out = cmd.output().context("spawning git")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn current_branch(root: &Path) -> Result<String> {
    let mut c = git_at(root);
    c.args(["rev-parse", "--abbrev-ref", "HEAD"]);
    let s = run(c)?.trim().to_string();
    if s.is_empty() || s == "HEAD" {
        return Err(anyhow!("detached head"));
    }
    Ok(s)
}

fn head_short(root: &Path) -> Result<String> {
    let mut c = git_at(root);
    c.args(["rev-parse", "--short", "HEAD"]);
    Ok(run(c)?.trim().to_string())
}

fn resolve_worktree_root(root: &Path) -> Result<PathBuf> {
    let mut c = git_at(root);
    c.args(["rev-parse", "--show-toplevel"]);
    let s = run(c)?.trim().to_string();
    Ok(PathBuf::from(s))
}

fn list_worktrees(root: &Path, current: &Path) -> Result<Vec<WorktreeEntry>> {
    let mut c = git_at(root);
    c.args(["worktree", "list", "--porcelain"]);
    let text = run(c)?;

    let mut result = Vec::new();
    let mut cur_path: Option<PathBuf> = None;
    let mut cur_branch: Option<String> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(p) = cur_path.take() {
                let is_current = p == current;
                result.push(WorktreeEntry {
                    path: p,
                    branch: cur_branch.take(),
                    is_current,
                });
            }
            cur_path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            cur_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        }
    }
    if let Some(p) = cur_path {
        let is_current = p == current;
        result.push(WorktreeEntry {
            path: p,
            branch: cur_branch,
            is_current,
        });
    }
    Ok(result)
}

fn list_local_branches(root: &Path) -> Result<Vec<String>> {
    let mut c = git_at(root);
    c.args(["branch", "--format=%(refname:short)"]);
    Ok(run(c)?
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

fn porcelain_status(root: &Path, worktree: &Path) -> Result<Vec<StatusEntry>> {
    let mut c = git_at(root);
    c.args(["status", "--porcelain=v1", "-uall"]);
    let text = run(c)?;
    let mut result = Vec::new();
    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        let bytes = line.as_bytes();
        let staged = bytes[0] as char;
        let unstaged = bytes[1] as char;
        let rest = &line[3..];
        let path_part = if let Some(idx) = rest.find(" -> ") {
            &rest[idx + 4..]
        } else {
            rest
        };
        result.push(StatusEntry {
            path: worktree.join(path_part),
            kind: classify(staged, unstaged),
        });
    }
    Ok(result)
}

fn classify(staged: char, unstaged: char) -> StatusKind {
    // Conflict signals come first; see git-status(1) porcelain v1.
    if staged == 'U'
        || unstaged == 'U'
        || (staged == 'A' && unstaged == 'A')
        || (staged == 'D' && unstaged == 'D')
    {
        return StatusKind::Conflicted;
    }
    if staged == '?' && unstaged == '?' {
        return StatusKind::Untracked;
    }
    if staged == '!' && unstaged == '!' {
        return StatusKind::Ignored;
    }
    // Unstaged takes priority when it's changed, because that's what the user's working on.
    let c = if unstaged != ' ' { unstaged } else { staged };
    match c {
        'M' | 'T' => StatusKind::Modified,
        'A' => StatusKind::Added,
        'D' => StatusKind::Deleted,
        'R' | 'C' => StatusKind::Renamed,
        _ => StatusKind::Modified,
    }
}

/// Diff between the file's on-disk contents and its HEAD blob, formatted
/// as a unified diff's line sequence (with hunk headers).
pub fn diff_vs_head(root: &Path, file: &Path) -> Result<Vec<DiffLine>> {
    let rel = file.strip_prefix(root).unwrap_or(file);
    // HEAD version — may not exist if file is newly added. Treat that as empty.
    let mut show = git_at(root);
    show.arg("show").arg(format!("HEAD:{}", rel.display()));
    let old = show.output().context("spawning git show")?;
    let old_text = if old.status.success() {
        String::from_utf8_lossy(&old.stdout).into_owned()
    } else {
        String::new()
    };
    let new_text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;

    let diff = TextDiff::from_lines(&old_text, &new_text);
    let mut lines = Vec::new();
    for group in diff.grouped_ops(3).iter() {
        let Some(first) = group.first() else {
            continue;
        };
        let Some(last) = group.last() else {
            continue;
        };
        let old_start = first.old_range().start + 1;
        let new_start = first.new_range().start + 1;
        let old_len = last.old_range().end.saturating_sub(first.old_range().start);
        let new_len = last.new_range().end.saturating_sub(first.new_range().start);
        lines.push(DiffLine {
            kind: DiffLineKind::HunkHeader,
            content: format!("@@ -{old_start},{old_len} +{new_start},{new_len} @@"),
            old_lineno: None,
            new_lineno: None,
        });
        for op in group.iter() {
            for change in diff.iter_changes(op) {
                let (kind, old_ln, new_ln) = match change.tag() {
                    ChangeTag::Equal => (
                        DiffLineKind::Context,
                        change.old_index().map(|i| i as u32 + 1),
                        change.new_index().map(|i| i as u32 + 1),
                    ),
                    ChangeTag::Delete => (
                        DiffLineKind::Removed,
                        change.old_index().map(|i| i as u32 + 1),
                        None,
                    ),
                    ChangeTag::Insert => (
                        DiffLineKind::Added,
                        None,
                        change.new_index().map(|i| i as u32 + 1),
                    ),
                };
                let content = change
                    .value()
                    .trim_end_matches('\n')
                    .trim_end_matches('\r')
                    .to_string();
                lines.push(DiffLine {
                    kind,
                    content,
                    old_lineno: old_ln,
                    new_lineno: new_ln,
                });
            }
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_handles_common_cases() {
        assert_eq!(classify('?', '?'), StatusKind::Untracked);
        assert_eq!(classify('!', '!'), StatusKind::Ignored);
        assert_eq!(classify(' ', 'M'), StatusKind::Modified);
        assert_eq!(classify('M', ' '), StatusKind::Modified);
        assert_eq!(classify(' ', 'D'), StatusKind::Deleted);
        assert_eq!(classify('A', ' '), StatusKind::Added);
        assert_eq!(classify('U', 'U'), StatusKind::Conflicted);
        assert_eq!(classify('A', 'A'), StatusKind::Conflicted);
        assert_eq!(classify('R', ' '), StatusKind::Renamed);
    }

    #[test]
    fn snapshot_on_hitled_repo_works() {
        // This test runs inside the hitled git repo itself.
        let cwd = std::env::current_dir().unwrap();
        let snap = snapshot(&cwd).expect("hitled repo should be snapshottable");
        assert!(
            snap.branch.is_some(),
            "should have a branch name on non-detached head"
        );
        assert!(snap.head_short.is_some(), "should have a head SHA");
        assert!(!snap.worktrees.is_empty(), "at least the current worktree");
        assert!(
            snap.worktrees.iter().any(|w| w.is_current),
            "exactly one current worktree"
        );
    }
}
