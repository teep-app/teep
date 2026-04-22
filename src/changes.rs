use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub path: PathBuf,
    // Timestamps retained for future sidebar display ("modified 3s ago"); not rendered in M2.
    #[allow(dead_code)]
    pub first_seen: Instant,
    #[allow(dead_code)]
    pub last_modified: Instant,
    pub seen_by_user: bool,
}

#[derive(Default)]
pub struct ChangeLog {
    entries: Vec<ChangedFile>,
    index: HashMap<PathBuf, usize>,
}

impl ChangeLog {
    pub fn record(&mut self, path: PathBuf) {
        let now = Instant::now();
        if let Some(&idx) = self.index.get(&path) {
            self.entries[idx].last_modified = now;
            // Re-modified -> unread again.
            self.entries[idx].seen_by_user = false;
        } else {
            self.index.insert(path.clone(), self.entries.len());
            self.entries.push(ChangedFile {
                path,
                first_seen: now,
                last_modified: now,
                seen_by_user: false,
            });
        }
    }

    pub fn entries(&self) -> &[ChangedFile] {
        &self.entries
    }

    pub fn unseen_count(&self) -> usize {
        self.entries.iter().filter(|c| !c.seen_by_user).count()
    }

    pub fn mark_seen(&mut self, path: &Path) {
        if let Some(&idx) = self.index.get(path) {
            self.entries[idx].seen_by_user = true;
        }
    }

    pub fn checkpoint(&mut self) {
        for c in &mut self.entries {
            c.seen_by_user = true;
        }
    }

    /// Next unseen entry after `current` (wrapping). If nothing is unseen,
    /// returns None. If `current` is None or not present, returns the first
    /// unseen entry.
    pub fn next_unseen_after(&self, current: Option<&Path>) -> Option<&ChangedFile> {
        let unseen: Vec<&ChangedFile> = self.entries.iter().filter(|c| !c.seen_by_user).collect();
        if unseen.is_empty() {
            return None;
        }
        let start = current
            .and_then(|p| unseen.iter().position(|c| c.path == p))
            .map(|i| (i + 1) % unseen.len())
            .unwrap_or(0);
        Some(unseen[start])
    }

    pub fn prev_unseen_before(&self, current: Option<&Path>) -> Option<&ChangedFile> {
        let unseen: Vec<&ChangedFile> = self.entries.iter().filter(|c| !c.seen_by_user).collect();
        if unseen.is_empty() {
            return None;
        }
        let start = current
            .and_then(|p| unseen.iter().position(|c| c.path == p))
            .map(|i| (i + unseen.len() - 1) % unseen.len())
            .unwrap_or(unseen.len() - 1);
        Some(unseen[start])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_dedupes_and_updates_last_modified() {
        let mut log = ChangeLog::default();
        log.record(PathBuf::from("a.rs"));
        log.record(PathBuf::from("a.rs"));
        assert_eq!(log.entries().len(), 1);
    }

    #[test]
    fn checkpoint_marks_all_seen() {
        let mut log = ChangeLog::default();
        log.record(PathBuf::from("a.rs"));
        log.record(PathBuf::from("b.rs"));
        log.checkpoint();
        assert_eq!(log.unseen_count(), 0);
    }

    #[test]
    fn next_unseen_cycles() {
        let mut log = ChangeLog::default();
        log.record(PathBuf::from("a.rs"));
        log.record(PathBuf::from("b.rs"));
        let first = log.next_unseen_after(None).unwrap().path.clone();
        let second = log.next_unseen_after(Some(&first)).unwrap().path.clone();
        let third = log.next_unseen_after(Some(&second)).unwrap().path.clone();
        assert_eq!(first, PathBuf::from("a.rs"));
        assert_eq!(second, PathBuf::from("b.rs"));
        assert_eq!(third, PathBuf::from("a.rs"), "should wrap around");
    }

    #[test]
    fn re_modifying_resets_seen() {
        let mut log = ChangeLog::default();
        log.record(PathBuf::from("a.rs"));
        log.checkpoint();
        assert_eq!(log.unseen_count(), 0);
        log.record(PathBuf::from("a.rs"));
        assert_eq!(log.unseen_count(), 1);
    }
}
