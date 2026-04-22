use std::{
    io::Stdout,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect, text::Line};
use tui_textarea::{Input, TextArea};

use crate::{
    changes::ChangeLog,
    commands::{CommandAction, PaletteState},
    config::Config,
    event::EventLoop,
    finder::{self, FinderState},
    fs_watch,
    git::{DiffLine, GitSnapshot, WorktreeEntry},
    runtime::Runtime,
    tree::{self, NodeKind, Tree},
};

pub struct AppState {
    pub root: PathBuf,
    #[allow(dead_code)]
    pub config: Config,
    pub quit: bool,
    pub last_ctrl_c: Option<Instant>,

    pub tree: Tree,
    pub tree_dirty: bool,
    pub last_tree_rebuild: Instant,
    pub changes: ChangeLog,
    pub open_file: Option<OpenFile>,
    pub focus: Focus,
    pub sidebar_visible: bool,
    pub status: Option<(String, Instant)>,
    /// Populated by `ui::view` each frame so `handle_mouse` can route clicks
    /// back to the thing that was visible at that screen cell.
    pub mouse_layout: MouseLayout,
    pub overlay: Overlay,

    // M4: git
    pub git_snapshot: Option<GitSnapshot>,
    pub git_dirty: bool,
    pub last_git_refresh: Instant,
    /// Set by `Msg::ReRootRequested`; the outer `run` loop observes this and
    /// restarts the session at the new path.
    pub reroot_to: Option<PathBuf>,

    // M5: edit mode — when we save via Cmd::SaveFile, the fs-watcher fires an
    // event for our own write. Suppress exactly one fs-change on that path so
    // it doesn't trip the conflict banner.
    pub ignore_next_fs_change: Option<PathBuf>,
}

/// Modal overlays that steal keyboard focus. Triggered by `/`, `:`, `?`, `g`, `b`.
#[derive(Default)]
pub enum Overlay {
    #[default]
    None,
    Finder(FinderState),
    Palette(PaletteState),
    Help,
    GitStatus,
    WorktreeSwitcher(WorktreeSwitcherState),
}

pub struct WorktreeSwitcherState {
    pub worktrees: Vec<WorktreeEntry>,
    pub selected: usize,
}

impl WorktreeSwitcherState {
    pub fn new(worktrees: Vec<WorktreeEntry>) -> Self {
        let selected = worktrees.iter().position(|w| w.is_current).unwrap_or(0);
        Self {
            worktrees,
            selected,
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.worktrees.len() {
            self.selected += 1;
        }
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

impl Overlay {
    pub fn is_active(&self) -> bool {
        !matches!(self, Overlay::None)
    }
}

#[derive(Default, Clone)]
pub struct MouseLayout {
    pub viewer: Rect,
    pub tree_rows: Vec<(u16, PathBuf)>,
    pub change_rows: Vec<(u16, PathBuf)>,
    /// Absolute column where the viewer begins. Clicks at col < this are sidebar-bound.
    pub viewer_col_min: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Viewer,
}

pub struct OpenFile {
    pub path: PathBuf,
    pub text: String,
    pub highlighted: Arc<Vec<Line<'static>>>,
    pub scroll: usize,
    pub error: Option<String>,

    // M4: diff view
    pub diff_mode: bool,
    pub diff: Option<Arc<Vec<DiffLine>>>,
    pub diff_error: Option<String>,

    // M5: edit state
    pub edit: EditState,

    // M6: markdown preview
    pub preview_mode: bool,
    pub rendered_markdown: Option<Arc<Vec<Line<'static>>>>,
}

/// True for file extensions we treat as markdown for preview purposes.
pub fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md" | "markdown" | "mdown" | "mkd" | "mkdn")
    )
}

#[derive(Default)]
pub enum EditState {
    #[default]
    View,
    Edit(EditBuffer),
    Conflict {
        buffer: EditBuffer,
    },
    Deleted {
        buffer: EditBuffer,
    },
}

pub struct EditBuffer {
    pub textarea: TextArea<'static>,
    /// Text as it was on disk when we entered (or last saved) — used to detect "dirty".
    pub base_text: String,
}

impl EditBuffer {
    pub fn new(initial: &str) -> Self {
        let lines: Vec<String> = initial.split('\n').map(|s| s.to_string()).collect();
        Self {
            textarea: TextArea::new(lines),
            base_text: initial.to_string(),
        }
    }

    pub fn current_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn is_dirty(&self) -> bool {
        self.current_text() != self.base_text
    }
}

pub struct LoadedFile {
    pub text: String,
    pub highlighted: Arc<Vec<Line<'static>>>,
    pub rendered_markdown: Option<Arc<Vec<Line<'static>>>>,
}

pub enum Msg {
    Key(KeyEvent),
    Mouse(MouseEvent),
    #[allow(dead_code)] // handlers land as we use the dimensions in later UI work
    Resize(u16, u16),
    FsChanged(Vec<PathBuf>),
    FileLoaded {
        path: PathBuf,
        result: Result<LoadedFile, String>,
    },
    FileSaved {
        path: PathBuf,
        result: Result<(), String>,
    },
    TreeRebuilt(tree::Node),
    GitRefreshed(Result<GitSnapshot, String>),
    DiffReady {
        path: PathBuf,
        result: Result<Vec<DiffLine>, String>,
    },
    ReRootRequested(PathBuf),
    Tick,
}

#[derive(Debug)]
pub enum Cmd {
    LoadFile(PathBuf),
    SaveFile { path: PathBuf, content: String },
    RebuildTree,
    RefreshGit,
    ComputeDiff(PathBuf),
    ReRoot(PathBuf),
}

const CTRL_C_QUIT_WINDOW: Duration = Duration::from_millis(1000);
const STATUS_LIFETIME: Duration = Duration::from_secs(2);
const PAGE_SCROLL: usize = 20;
const TREE_REBUILD_THROTTLE: Duration = Duration::from_millis(500);
const GIT_REFRESH_ON_DIRTY: Duration = Duration::from_secs(1);
const GIT_REFRESH_MAX_INTERVAL: Duration = Duration::from_secs(5);

pub fn update(state: &mut AppState, msg: Msg) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    match msg {
        Msg::Key(key) => handle_key(state, key, &mut cmds),
        Msg::Mouse(ev) => handle_mouse(state, ev, &mut cmds),
        Msg::FsChanged(paths) => handle_fs_changed(state, paths, &mut cmds),
        Msg::FileLoaded { path, result } => handle_file_loaded(state, path, result),
        Msg::FileSaved { path, result } => handle_file_saved(state, path, result),
        Msg::TreeRebuilt(node) => state.tree.graft(node),
        Msg::GitRefreshed(result) => handle_git_refreshed(state, result),
        Msg::DiffReady { path, result } => handle_diff_ready(state, path, result),
        Msg::ReRootRequested(path) => state.reroot_to = Some(path),
        Msg::Tick => handle_tick(state, &mut cmds),
        Msg::Resize(_, _) => {}
    }
    cmds
}

fn handle_key(state: &mut AppState, key: KeyEvent, cmds: &mut Vec<Cmd>) {
    let is_ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
    if is_ctrl_c {
        handle_ctrl_c(state);
        return;
    }
    state.last_ctrl_c = None;

    // Overlays own the keyboard while open.
    if state.overlay.is_active() {
        handle_overlay_key(state, key, cmds);
        return;
    }

    // Edit mode + banner states own the keyboard next.
    let edit_kind = state.open_file.as_ref().map(|f| match &f.edit {
        EditState::View => 0u8,
        EditState::Edit(_) => 1,
        EditState::Conflict { .. } => 2,
        EditState::Deleted { .. } => 3,
    });
    match edit_kind {
        Some(1) => {
            handle_edit_key(state, key, cmds);
            return;
        }
        Some(2) => {
            handle_conflict_key(state, key, cmds);
            return;
        }
        Some(3) => {
            handle_deleted_key(state, key, cmds);
            return;
        }
        _ => {}
    }

    // Open an overlay.
    match (key.code, key.modifiers) {
        (KeyCode::Char('/'), m) if !m.contains(KeyModifiers::CONTROL) => {
            let items = finder::items_from_tree(&state.tree.root);
            state.overlay = Overlay::Finder(FinderState::new(items));
            return;
        }
        (KeyCode::Char(':'), _) => {
            state.overlay = Overlay::Palette(PaletteState::new());
            return;
        }
        (KeyCode::Char('?'), _) => {
            state.overlay = Overlay::Help;
            return;
        }
        _ => {}
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('b'), m) if m.contains(KeyModifiers::CONTROL) => {
            state.sidebar_visible = !state.sidebar_visible;
        }
        (KeyCode::Tab, _) => {
            state.focus = match state.focus {
                Focus::Tree => Focus::Viewer,
                Focus::Viewer => Focus::Tree,
            };
        }
        (KeyCode::Char('n'), m) if !m.contains(KeyModifiers::SHIFT) => {
            cycle_changes(state, true, cmds);
        }
        (KeyCode::Char('N'), _) => cycle_changes(state, false, cmds),
        (KeyCode::Char('u'), _) => {
            state.changes.checkpoint();
            set_status(state, "changes: all seen".to_string());
        }
        (KeyCode::Char('r'), _) => {
            state.tree_dirty = false;
            state.last_tree_rebuild = Instant::now();
            cmds.push(Cmd::RebuildTree);
            set_status(state, "refreshing tree...".to_string());
        }
        (KeyCode::Char('d'), _) => toggle_diff(state, cmds),
        (KeyCode::Char('m'), _) => toggle_preview(state),
        (KeyCode::Char('g'), _) => open_git_status(state),
        (KeyCode::Char('b'), _) => open_worktree_switcher(state),
        (KeyCode::Char('i'), _) | (KeyCode::Char('e'), _) => enter_edit_mode(state),
        _ => match state.focus {
            Focus::Tree => handle_tree_key(state, key, cmds),
            Focus::Viewer => handle_viewer_key(state, key),
        },
    }
}

fn enter_edit_mode(state: &mut AppState) {
    let Some(open) = state.open_file.as_mut() else {
        set_status(state, "no file open".to_string());
        return;
    };
    if open.error.is_some() {
        set_status(state, "cannot edit: file has errors".to_string());
        return;
    }
    if !matches!(open.edit, EditState::View) {
        return;
    }
    // Edit and diff are mutually exclusive views.
    open.diff_mode = false;
    let buffer = EditBuffer::new(&open.text);
    open.edit = EditState::Edit(buffer);
    state.focus = Focus::Viewer;
}

fn handle_edit_key(state: &mut AppState, key: KeyEvent, cmds: &mut Vec<Cmd>) {
    if key.code == KeyCode::Esc {
        // Exit edit mode. If dirty, discard changes (user must Ctrl-S to save).
        let Some(open) = state.open_file.as_mut() else {
            return;
        };
        let was_dirty = match &open.edit {
            EditState::Edit(b) => b.is_dirty(),
            _ => false,
        };
        open.edit = EditState::View;
        if was_dirty {
            set_status(state, "discarded unsaved edits".to_string());
        }
        return;
    }
    if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
        save_current(state, cmds);
        return;
    }
    // Everything else flows into the text area.
    let Some(open) = state.open_file.as_mut() else {
        return;
    };
    if let EditState::Edit(buffer) = &mut open.edit {
        let input: Input = Input::from(key);
        buffer.textarea.input(input);
    }
}

fn save_current(state: &mut AppState, cmds: &mut Vec<Cmd>) {
    let Some(open) = state.open_file.as_mut() else {
        return;
    };
    let EditState::Edit(buffer) = &mut open.edit else {
        return;
    };
    let mut content = buffer.current_text();
    // Conventionally, text files end with a newline; respect base_text's convention.
    if !content.ends_with('\n') && buffer.base_text.ends_with('\n') {
        content.push('\n');
    }
    // Update base so our own fs-event doesn't look like a conflict.
    buffer.base_text = content.clone();
    let path = open.path.clone();
    state.ignore_next_fs_change = Some(path.clone());
    cmds.push(Cmd::SaveFile { path, content });
}

fn handle_conflict_key(state: &mut AppState, key: KeyEvent, cmds: &mut Vec<Cmd>) {
    let Some(open) = state.open_file.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('k') | KeyCode::Esc => {
            // Keep my edits; continue editing. Next save will clobber the agent's write.
            if let EditState::Conflict { buffer } = std::mem::take(&mut open.edit) {
                open.edit = EditState::Edit(buffer);
            }
            set_status(state, "kept your edits".to_string());
        }
        KeyCode::Char('t') => {
            // Take theirs: discard edits, reload.
            open.edit = EditState::View;
            let path = open.path.clone();
            cmds.push(Cmd::LoadFile(path));
            set_status(state, "reloaded from disk".to_string());
        }
        _ => {}
    }
}

fn handle_deleted_key(state: &mut AppState, key: KeyEvent, cmds: &mut Vec<Cmd>) {
    match key.code {
        KeyCode::Char('r') => {
            let Some(open) = state.open_file.as_mut() else {
                return;
            };
            let EditState::Deleted { buffer } = std::mem::take(&mut open.edit) else {
                return;
            };
            let content = buffer.current_text();
            let path = open.path.clone();
            state.ignore_next_fs_change = Some(path.clone());
            cmds.push(Cmd::SaveFile {
                path,
                content: content.clone(),
            });
            // Go back to edit mode with the rewritten file as our new base.
            let mut new_buffer = EditBuffer::new(&content);
            new_buffer.base_text = content;
            open.edit = EditState::Edit(new_buffer);
            set_status(state, "restoring file...".to_string());
        }
        KeyCode::Char('c') | KeyCode::Esc => {
            state.open_file = None;
            set_status(state, "closed".to_string());
        }
        _ => {}
    }
}

fn handle_file_saved(state: &mut AppState, path: PathBuf, result: Result<(), String>) {
    match result {
        Ok(()) => set_status(state, format!("saved {}", path.display())),
        Err(e) => {
            // Our suppression is no longer valid since the write didn't land.
            state.ignore_next_fs_change = None;
            set_status(state, format!("save failed: {e}"));
        }
    }
}

fn toggle_preview(state: &mut AppState) {
    let Some(open) = state.open_file.as_mut() else {
        set_status(state, "no file open".to_string());
        return;
    };
    if !is_markdown_path(&open.path) {
        set_status(state, "preview is only for markdown files".to_string());
        return;
    }
    open.preview_mode = !open.preview_mode;
    // Entering preview mode exits diff mode — the two viewer layouts don't combine.
    if open.preview_mode {
        open.diff_mode = false;
        // Reset scroll — rendered line count != source line count.
        open.scroll = 0;
    }
}

fn toggle_diff(state: &mut AppState, cmds: &mut Vec<Cmd>) {
    let Some(f) = state.open_file.as_mut() else {
        set_status(state, "no file open".to_string());
        return;
    };
    f.diff_mode = !f.diff_mode;
    if f.diff_mode && f.diff.is_none() && f.diff_error.is_none() {
        cmds.push(Cmd::ComputeDiff(f.path.clone()));
    }
}

fn open_git_status(state: &mut AppState) {
    if state.git_snapshot.is_none() {
        set_status(state, "not a git repository".to_string());
        return;
    }
    state.overlay = Overlay::GitStatus;
}

fn open_worktree_switcher(state: &mut AppState) {
    let Some(snap) = state.git_snapshot.as_ref() else {
        set_status(state, "not a git repository".to_string());
        return;
    };
    if snap.worktrees.is_empty() {
        set_status(state, "no worktrees".to_string());
        return;
    }
    state.overlay = Overlay::WorktreeSwitcher(WorktreeSwitcherState::new(snap.worktrees.clone()));
}

fn handle_ctrl_c(state: &mut AppState) {
    let now = Instant::now();
    match state.last_ctrl_c {
        Some(prev) if now.duration_since(prev) <= CTRL_C_QUIT_WINDOW => {
            state.quit = true;
        }
        _ => {
            state.last_ctrl_c = Some(now);
        }
    }
}

fn handle_tree_key(state: &mut AppState, key: KeyEvent, cmds: &mut Vec<Cmd>) {
    match key.code {
        KeyCode::Down => state.tree.move_down(),
        KeyCode::Up => state.tree.move_up(),
        KeyCode::Right => state.tree.expand_selected(),
        KeyCode::Left => state.tree.collapse_selected(),
        KeyCode::Enter | KeyCode::Char('o') => {
            if let Some(node) = state.tree.selected_node() {
                let kind = node.kind;
                let path = node.path.clone();
                match kind {
                    NodeKind::Dir => state.tree.toggle_selected(),
                    NodeKind::File => cmds.push(Cmd::LoadFile(path)),
                }
            }
        }
        _ => {}
    }
}

fn handle_viewer_key(state: &mut AppState, key: KeyEvent) {
    let Some(f) = state.open_file.as_mut() else {
        return;
    };
    let total = f.highlighted.len();
    let last = total.saturating_sub(1);
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            if f.scroll < last {
                f.scroll += 1;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            f.scroll = f.scroll.saturating_sub(1);
        }
        KeyCode::PageDown => {
            f.scroll = (f.scroll + PAGE_SCROLL).min(last);
        }
        KeyCode::PageUp => {
            f.scroll = f.scroll.saturating_sub(PAGE_SCROLL);
        }
        KeyCode::Home => f.scroll = 0,
        KeyCode::End => f.scroll = last,
        _ => {}
    }
}

fn handle_overlay_key(state: &mut AppState, key: KeyEvent, cmds: &mut Vec<Cmd>) {
    match key.code {
        KeyCode::Esc => state.overlay = Overlay::None,
        KeyCode::Up => match &mut state.overlay {
            Overlay::Finder(f) => f.move_up(),
            Overlay::Palette(p) => p.move_up(),
            Overlay::WorktreeSwitcher(w) => w.move_up(),
            Overlay::Help | Overlay::GitStatus | Overlay::None => {}
        },
        KeyCode::Down => match &mut state.overlay {
            Overlay::Finder(f) => f.move_down(),
            Overlay::Palette(p) => p.move_down(),
            Overlay::WorktreeSwitcher(w) => w.move_down(),
            Overlay::Help | Overlay::GitStatus | Overlay::None => {}
        },
        KeyCode::Backspace => match &mut state.overlay {
            Overlay::Finder(f) => f.pop(),
            Overlay::Palette(p) => p.pop(),
            _ => {}
        },
        KeyCode::Enter => match std::mem::replace(&mut state.overlay, Overlay::None) {
            Overlay::Finder(f) => {
                if let Some(p) = f.selected_path() {
                    cmds.push(Cmd::LoadFile(p));
                }
            }
            Overlay::Palette(p) => {
                if let Some(cmd) = p.selected_command() {
                    execute_command_action(state, cmd.action, cmds);
                }
            }
            Overlay::WorktreeSwitcher(w) => {
                if let Some(entry) = w.worktrees.get(w.selected) {
                    if entry.is_current {
                        set_status(state, "already at this worktree".to_string());
                    } else {
                        cmds.push(Cmd::ReRoot(entry.path.clone()));
                    }
                }
            }
            Overlay::Help | Overlay::GitStatus | Overlay::None => {}
        },
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            match &mut state.overlay {
                Overlay::Finder(f) => f.push(c),
                Overlay::Palette(p) => p.push(c),
                Overlay::Help | Overlay::GitStatus => state.overlay = Overlay::None,
                Overlay::WorktreeSwitcher(_) | Overlay::None => {}
            }
        }
        _ => {}
    }
}

fn execute_command_action(state: &mut AppState, action: CommandAction, cmds: &mut Vec<Cmd>) {
    match action {
        CommandAction::ToggleSidebar => state.sidebar_visible = !state.sidebar_visible,
        CommandAction::RefreshTree => {
            state.tree_dirty = false;
            state.last_tree_rebuild = Instant::now();
            cmds.push(Cmd::RebuildTree);
            set_status(state, "refreshing tree...".to_string());
        }
        CommandAction::CheckpointChanges => {
            state.changes.checkpoint();
            set_status(state, "changes: all seen".to_string());
        }
        CommandAction::ShowHelp => state.overlay = Overlay::Help,
        CommandAction::GitStatus => open_git_status(state),
        CommandAction::Worktrees => open_worktree_switcher(state),
        CommandAction::Quit => state.quit = true,
    }
}

fn handle_mouse(state: &mut AppState, ev: MouseEvent, cmds: &mut Vec<Cmd>) {
    // Any mouse interaction while an overlay is open dismisses it.
    if state.overlay.is_active() {
        if matches!(ev.kind, MouseEventKind::Down(_)) {
            state.overlay = Overlay::None;
        }
        return;
    }

    let col = ev.column;
    let row = ev.row;
    let in_sidebar = col < state.mouse_layout.viewer_col_min;

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if in_sidebar {
                if let Some((_, path)) = state
                    .mouse_layout
                    .tree_rows
                    .iter()
                    .find(|(r, _)| *r == row)
                    .cloned()
                {
                    let was_selected = state.tree.selected == path;
                    state.tree.selected = path.clone();
                    state.focus = Focus::Tree;
                    if was_selected && let Some(node) = state.tree.selected_node() {
                        let kind = node.kind;
                        match kind {
                            NodeKind::Dir => state.tree.toggle_selected(),
                            NodeKind::File => cmds.push(Cmd::LoadFile(path)),
                        }
                    }
                    return;
                }
                if let Some((_, path)) = state
                    .mouse_layout
                    .change_rows
                    .iter()
                    .find(|(r, _)| *r == row)
                    .cloned()
                {
                    cmds.push(Cmd::LoadFile(path));
                    return;
                }
                // Empty sidebar click — just focus the tree pane.
                state.focus = Focus::Tree;
            } else if contains(&state.mouse_layout.viewer, col, row) {
                state.focus = Focus::Viewer;
            }
        }
        MouseEventKind::ScrollDown => {
            if in_sidebar {
                state.tree.move_down();
            } else if let Some(f) = state.open_file.as_mut() {
                let last = f.highlighted.len().saturating_sub(1);
                if f.scroll < last {
                    f.scroll += 1;
                }
            }
        }
        MouseEventKind::ScrollUp => {
            if in_sidebar {
                state.tree.move_up();
            } else if let Some(f) = state.open_file.as_mut() {
                f.scroll = f.scroll.saturating_sub(1);
            }
        }
        _ => {}
    }
}

fn contains(rect: &Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn cycle_changes(state: &mut AppState, forward: bool, cmds: &mut Vec<Cmd>) {
    let current = state.open_file.as_ref().map(|f| f.path.as_path());
    let next = if forward {
        state.changes.next_unseen_after(current)
    } else {
        state.changes.prev_unseen_before(current)
    };
    if let Some(cf) = next {
        cmds.push(Cmd::LoadFile(cf.path.clone()));
    } else {
        set_status(state, "no unseen changes".to_string());
    }
}

fn handle_fs_changed(state: &mut AppState, paths: Vec<PathBuf>, cmds: &mut Vec<Cmd>) {
    let mut touched_anything = false;
    for path in paths {
        // `.git/` events are change-log noise but a strong git-refresh signal.
        if path.components().any(|c| c.as_os_str() == ".git") {
            state.git_dirty = true;
            continue;
        }
        if is_noise(&path) {
            continue;
        }

        // Skip fs events we triggered ourselves via Cmd::SaveFile so our own
        // write doesn't look like an external conflict.
        if state.ignore_next_fs_change.as_ref() == Some(&path) {
            state.ignore_next_fs_change = None;
            continue;
        }

        let metadata = path.metadata();
        let exists = metadata.is_ok();
        let is_dir = matches!(metadata, Ok(ref m) if m.is_dir());

        if !exists {
            // Only meaningful when the vanished path is the currently-open file.
            // Otherwise it's almost always an atomic-rename artifact and the
            // follow-up event gives us the real file.
            if state.open_file.as_ref().is_some_and(|f| f.path == path) {
                handle_open_file_deleted(state);
            }
            continue;
        }
        if is_dir {
            continue;
        }

        touched_anything = true;
        state.changes.record(path.clone());

        if let Some(open) = state.open_file.as_mut()
            && open.path == path
        {
            match std::mem::take(&mut open.edit) {
                EditState::Edit(buffer) | EditState::Conflict { buffer } => {
                    // Any external write while we're mid-edit is a conflict.
                    open.edit = EditState::Conflict { buffer };
                }
                EditState::Deleted { buffer } => {
                    // File came back via a non-self-save event — treat as conflict.
                    open.edit = EditState::Conflict { buffer };
                }
                EditState::View => {
                    open.edit = EditState::View;
                    cmds.push(Cmd::LoadFile(path));
                }
            }
        }
    }
    if touched_anything {
        state.tree_dirty = true;
        state.git_dirty = true;
    }
}

fn handle_open_file_deleted(state: &mut AppState) {
    let Some(open) = state.open_file.as_mut() else {
        return;
    };
    match std::mem::take(&mut open.edit) {
        EditState::Edit(buffer) | EditState::Conflict { buffer } => {
            open.edit = EditState::Deleted { buffer };
        }
        EditState::Deleted { buffer } => {
            open.edit = EditState::Deleted { buffer };
        }
        EditState::View => {
            set_status(state, "file removed on disk".to_string());
            state.open_file = None;
        }
    }
}

/// Returns true for fs events we should not record or act on: dotfiles,
/// `.git/*`, common editor atomic-rename temp files. Existence is handled by
/// the caller so deletion of the open file can be observed.
pub(crate) fn is_noise(path: &Path) -> bool {
    for c in path.components() {
        let bytes = c.as_os_str().as_encoded_bytes();
        if bytes == b".git" {
            return true;
        }
        // Hidden (starts with '.'), but allow '.' and '..' traversal.
        if bytes.starts_with(b".") && bytes != b"." && bytes != b".." {
            return true;
        }
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.is_empty() {
        return true;
    }
    if name.contains(".tmp.") {
        return true;
    }
    if name.ends_with('~') || name.ends_with(".swp") || name.ends_with(".swo") {
        return true;
    }
    if name.starts_with(".#") {
        return true;
    }
    false
}

fn handle_file_loaded(state: &mut AppState, path: PathBuf, result: Result<LoadedFile, String>) {
    state.changes.mark_seen(&path);
    // Detect an auto-reload (same path already open) so we can surface the
    // "↻ reloaded" toast — otherwise silent reloads leave the user wondering
    // whether HITLed even noticed.
    let is_reload = state.open_file.as_ref().is_some_and(|f| f.path == path);
    // Reloading a file always invalidates its diff (it's vs HEAD of on-disk bytes).
    match result {
        Ok(loaded) => {
            let preserve_scroll = state
                .open_file
                .as_ref()
                .filter(|f| f.path == path)
                .map(|f| f.scroll.min(loaded.highlighted.len().saturating_sub(1)));
            let diff_mode = state
                .open_file
                .as_ref()
                .filter(|f| f.path == path)
                .is_some_and(|f| f.diff_mode);
            let preview_mode = state
                .open_file
                .as_ref()
                .filter(|f| f.path == path)
                .is_some_and(|f| f.preview_mode);
            state.open_file = Some(OpenFile {
                path,
                text: loaded.text,
                highlighted: loaded.highlighted,
                scroll: preserve_scroll.unwrap_or(0),
                error: None,
                diff_mode,
                diff: None,
                diff_error: None,
                edit: EditState::View,
                preview_mode,
                rendered_markdown: loaded.rendered_markdown,
            });
            state.focus = Focus::Viewer;
            if is_reload {
                set_status(state, "↻ reloaded".to_string());
            }
        }
        Err(e) => {
            let msg = format!("failed: {e}");
            state.open_file = Some(OpenFile {
                path,
                text: String::new(),
                highlighted: Arc::new(Vec::new()),
                scroll: 0,
                error: Some(msg.clone()),
                diff_mode: false,
                diff: None,
                diff_error: None,
                edit: EditState::View,
                preview_mode: false,
                rendered_markdown: None,
            });
            set_status(state, msg);
        }
    }
}

fn handle_git_refreshed(state: &mut AppState, result: Result<GitSnapshot, String>) {
    match result {
        Ok(snap) => {
            state.git_snapshot = Some(snap);
            state.last_git_refresh = Instant::now();
        }
        Err(e) => {
            // Not a git repo, or something else went wrong — drop the snapshot
            // silently rather than pestering the user. Logs will have details.
            tracing::debug!(error = %e, "git snapshot failed");
            state.git_snapshot = None;
            state.last_git_refresh = Instant::now();
        }
    }
}

fn handle_diff_ready(state: &mut AppState, path: PathBuf, result: Result<Vec<DiffLine>, String>) {
    // Only apply if the currently-open file still matches; otherwise the user
    // moved on and this diff is stale.
    let Some(open) = state.open_file.as_mut() else {
        return;
    };
    if open.path != path {
        return;
    }
    match result {
        Ok(lines) => {
            open.diff = Some(Arc::new(lines));
            open.diff_error = None;
        }
        Err(e) => {
            open.diff_error = Some(e);
        }
    }
}

fn handle_tick(state: &mut AppState, cmds: &mut Vec<Cmd>) {
    if let Some((_, at)) = &state.status
        && at.elapsed() > STATUS_LIFETIME
    {
        state.status = None;
    }
    if state.tree_dirty && state.last_tree_rebuild.elapsed() >= TREE_REBUILD_THROTTLE {
        state.tree_dirty = false;
        state.last_tree_rebuild = Instant::now();
        cmds.push(Cmd::RebuildTree);
    }
    // Git refresh: fire when dirty (after a short settle) OR periodically as a
    // sweep so we catch external git ops not signalled by fs-watch.
    let git_elapsed = state.last_git_refresh.elapsed();
    let should_refresh_git = (state.git_dirty && git_elapsed >= GIT_REFRESH_ON_DIRTY)
        || git_elapsed >= GIT_REFRESH_MAX_INTERVAL;
    if should_refresh_git {
        state.git_dirty = false;
        state.last_git_refresh = Instant::now();
        cmds.push(Cmd::RefreshGit);
    }
}

fn set_status(state: &mut AppState, msg: String) {
    state.status = Some((msg, Instant::now()));
}

enum SessionOutcome {
    Quit,
    Reroot(PathBuf),
}

pub async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    initial_root: PathBuf,
    config: Config,
) -> Result<()> {
    // Pre-warm the syntect OnceLocks once per process so the first real file
    // open isn't a visible pause. Idempotent across reroot sessions.
    std::thread::spawn(|| {
        crate::syntax::highlight_file("", std::path::Path::new("warmup.txt"));
    });

    let mut root = initial_root;
    loop {
        match run_session(terminal, &root, &config).await? {
            SessionOutcome::Quit => return Ok(()),
            SessionOutcome::Reroot(new_root) => {
                tracing::info!(from = %root.display(), to = %new_root.display(), "rerooting");
                root = new_root;
            }
        }
    }
}

async fn run_session(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    root: &Path,
    config: &Config,
) -> Result<SessionOutcome> {
    let tree = Tree::build(root)?;
    let mut state = AppState {
        root: root.to_path_buf(),
        config: config.clone(),
        quit: false,
        last_ctrl_c: None,
        tree,
        tree_dirty: false,
        last_tree_rebuild: Instant::now(),
        changes: ChangeLog::default(),
        open_file: None,
        focus: Focus::Tree,
        sidebar_visible: true,
        status: None,
        mouse_layout: MouseLayout::default(),
        overlay: Overlay::None,
        git_snapshot: None,
        git_dirty: false,
        // Back-date so the first tick emits a refresh immediately.
        last_git_refresh: Instant::now() - GIT_REFRESH_MAX_INTERVAL,
        reroot_to: None,
        ignore_next_fs_change: None,
    };

    let mut events = EventLoop::new();
    let runtime = Runtime::new(events.sender(), root.to_path_buf());
    let _fs_watcher = fs_watch::spawn(root.to_path_buf(), events.sender())?;

    while !state.quit && state.reroot_to.is_none() {
        terminal.draw(|f| crate::ui::view(&mut state, f))?;
        let Some(msg) = events.next().await else {
            break;
        };
        let cmds = update(&mut state, msg);
        for cmd in cmds {
            runtime.execute(cmd);
        }
    }

    if let Some(new_root) = state.reroot_to.take() {
        Ok(SessionOutcome::Reroot(new_root))
    } else {
        Ok(SessionOutcome::Quit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> AppState {
        let path = PathBuf::from("/tmp");
        AppState {
            root: path.clone(),
            config: Config::default(),
            quit: false,
            last_ctrl_c: None,
            tree: Tree::for_testing(path),
            tree_dirty: false,
            last_tree_rebuild: Instant::now(),
            changes: ChangeLog::default(),
            open_file: None,
            focus: Focus::Tree,
            sidebar_visible: true,
            status: None,
            mouse_layout: MouseLayout::default(),
            overlay: Overlay::None,
            git_snapshot: None,
            git_dirty: false,
            last_git_refresh: Instant::now(),
            reroot_to: None,
            ignore_next_fs_change: None,
        }
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn single_ctrl_c_arms_quit() {
        let mut s = fixture();
        update(&mut s, Msg::Key(ctrl_c()));
        assert!(!s.quit);
        assert!(s.last_ctrl_c.is_some());
    }

    #[test]
    fn two_fast_ctrl_c_events_quit() {
        let mut s = fixture();
        update(&mut s, Msg::Key(ctrl_c()));
        update(&mut s, Msg::Key(ctrl_c()));
        assert!(s.quit);
    }

    #[test]
    fn any_other_key_clears_pending_quit() {
        let mut s = fixture();
        update(&mut s, Msg::Key(ctrl_c()));
        update(&mut s, Msg::Key(plain('a')));
        assert!(s.last_ctrl_c.is_none());
    }

    #[test]
    fn tab_cycles_focus() {
        let mut s = fixture();
        assert_eq!(s.focus, Focus::Tree);
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        );
        assert_eq!(s.focus, Focus::Viewer);
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        );
        assert_eq!(s.focus, Focus::Tree);
    }

    #[test]
    fn ctrl_b_toggles_sidebar() {
        let mut s = fixture();
        assert!(s.sidebar_visible);
        update(&mut s, Msg::Key(ctrl('b')));
        assert!(!s.sidebar_visible);
        update(&mut s, Msg::Key(ctrl('b')));
        assert!(s.sidebar_visible);
    }

    fn make_test_file(suffix: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("hitled_test_{}_{}.rs", std::process::id(), suffix));
        std::fs::write(&p, "fn main(){}\n").unwrap();
        p
    }

    #[test]
    fn fs_change_records_and_reloads_open_file() {
        let tmpfile = make_test_file("records");
        let mut s = fixture();
        s.open_file = Some(OpenFile {
            path: tmpfile.clone(),
            text: String::new(),
            highlighted: Arc::new(Vec::new()),
            scroll: 0,
            error: None,
            diff_mode: false,
            diff: None,
            diff_error: None,
            edit: EditState::View,
            preview_mode: false,
            rendered_markdown: None,
        });
        let cmds = update(&mut s, Msg::FsChanged(vec![tmpfile.clone()]));
        assert_eq!(s.changes.entries().len(), 1);
        assert!(matches!(cmds.as_slice(), [Cmd::LoadFile(p)] if p == &tmpfile));
        assert!(s.tree_dirty, "relevant fs event should mark tree dirty");
        std::fs::remove_file(&tmpfile).ok();
    }

    #[test]
    fn fs_change_ignores_git_paths() {
        let mut s = fixture();
        update(
            &mut s,
            Msg::FsChanged(vec![PathBuf::from("/tmp/.git/index")]),
        );
        assert_eq!(s.changes.entries().len(), 0);
        assert!(!s.tree_dirty);
    }

    #[test]
    fn is_noise_rejects_the_things_it_should() {
        assert!(is_noise(Path::new("/tmp/.git/index")));
        assert!(is_noise(Path::new("/tmp/.gitignore")));
        assert!(is_noise(Path::new("/tmp/.claude/config")));
        assert!(is_noise(Path::new("/tmp/README.md.tmp.12902.17768")));
        assert!(is_noise(Path::new("/tmp/foo~")));
        assert!(is_noise(Path::new("/tmp/.foo.swp")));
        assert!(is_noise(Path::new("/tmp/.#emacslock")));
        // Nonexistent-but-otherwise-ok paths are NOT noise — handle_fs_changed
        // handles them as deletions of the open file.
        assert!(
            !is_noise(Path::new("/tmp/definitely_does_not_exist_hitled_test.rs")),
            "nonexistent paths are handled by handle_fs_changed, not is_noise"
        );
    }

    #[test]
    fn is_noise_accepts_real_files() {
        let tmpfile = make_test_file("accepts");
        assert!(!is_noise(&tmpfile));
        std::fs::remove_file(&tmpfile).ok();
    }

    #[test]
    fn tick_fires_rebuild_when_dirty_and_throttle_elapsed() {
        let mut s = fixture();
        s.tree_dirty = true;
        s.last_tree_rebuild = Instant::now() - Duration::from_secs(1);
        let cmds = update(&mut s, Msg::Tick);
        assert!(matches!(cmds.as_slice(), [Cmd::RebuildTree]));
        assert!(!s.tree_dirty, "dirty should be cleared on rebuild emit");
    }

    #[test]
    fn tick_does_not_fire_rebuild_within_throttle() {
        let mut s = fixture();
        s.tree_dirty = true;
        s.last_tree_rebuild = Instant::now();
        let cmds = update(&mut s, Msg::Tick);
        assert!(cmds.is_empty());
        assert!(s.tree_dirty, "dirty persists across throttled tick");
    }

    fn mouse_at(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn left_click_on_change_log_loads_file() {
        let tmpfile = make_test_file("mouse_click");
        let mut s = fixture();
        // Layout: viewer starts at col 30, change row at absolute row 5 maps to tmpfile.
        s.mouse_layout = MouseLayout {
            viewer: Rect {
                x: 30,
                y: 0,
                width: 50,
                height: 20,
            },
            viewer_col_min: 30,
            tree_rows: vec![],
            change_rows: vec![(5, tmpfile.clone())],
        };
        let cmds = update(
            &mut s,
            Msg::Mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), 10, 5)),
        );
        assert!(matches!(cmds.as_slice(), [Cmd::LoadFile(p)] if p == &tmpfile));
        std::fs::remove_file(&tmpfile).ok();
    }

    #[test]
    fn left_click_on_viewer_focuses_viewer() {
        let mut s = fixture();
        s.focus = Focus::Tree;
        s.mouse_layout = MouseLayout {
            viewer: Rect {
                x: 30,
                y: 0,
                width: 50,
                height: 20,
            },
            viewer_col_min: 30,
            tree_rows: vec![],
            change_rows: vec![],
        };
        update(
            &mut s,
            Msg::Mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), 40, 5)),
        );
        assert_eq!(s.focus, Focus::Viewer);
    }

    #[test]
    fn scroll_in_viewer_moves_scroll() {
        let mut s = fixture();
        s.focus = Focus::Viewer;
        s.open_file = Some(OpenFile {
            path: PathBuf::from("/tmp/a.rs"),
            text: String::new(),
            highlighted: Arc::new(vec![Line::from("1"), Line::from("2"), Line::from("3")]),
            scroll: 0,
            error: None,
            diff_mode: false,
            diff: None,
            diff_error: None,
            edit: EditState::View,
            preview_mode: false,
            rendered_markdown: None,
        });
        s.mouse_layout = MouseLayout {
            viewer: Rect {
                x: 30,
                y: 0,
                width: 50,
                height: 20,
            },
            viewer_col_min: 30,
            tree_rows: vec![],
            change_rows: vec![],
        };
        update(
            &mut s,
            Msg::Mouse(mouse_at(MouseEventKind::ScrollDown, 40, 5)),
        );
        assert_eq!(s.open_file.as_ref().unwrap().scroll, 1);
        update(
            &mut s,
            Msg::Mouse(mouse_at(MouseEventKind::ScrollUp, 40, 5)),
        );
        assert_eq!(s.open_file.as_ref().unwrap().scroll, 0);
    }

    #[test]
    fn r_key_triggers_immediate_rebuild() {
        let mut s = fixture();
        let cmds = update(&mut s, Msg::Key(plain('r')));
        assert!(matches!(cmds.as_slice(), [Cmd::RebuildTree]));
        assert!(s.status.is_some());
    }

    #[test]
    fn n_cycles_to_unseen_change() {
        let mut s = fixture();
        s.changes.record(PathBuf::from("/tmp/a.rs"));
        s.changes.record(PathBuf::from("/tmp/b.rs"));
        let cmds = update(&mut s, Msg::Key(plain('n')));
        assert!(matches!(cmds.as_slice(), [Cmd::LoadFile(p)] if p == &PathBuf::from("/tmp/a.rs")));
    }

    #[test]
    fn n_with_no_unseen_shows_status() {
        let mut s = fixture();
        let cmds = update(&mut s, Msg::Key(plain('n')));
        assert!(cmds.is_empty());
        assert!(s.status.is_some());
    }

    #[test]
    fn u_checkpoints_changes() {
        let mut s = fixture();
        s.changes.record(PathBuf::from("/tmp/a.rs"));
        assert_eq!(s.changes.unseen_count(), 1);
        update(&mut s, Msg::Key(plain('u')));
        assert_eq!(s.changes.unseen_count(), 0);
        assert!(s.status.is_some());
    }

    #[test]
    fn file_loaded_stores_and_focuses_viewer() {
        let mut s = fixture();
        s.focus = Focus::Tree;
        update(
            &mut s,
            Msg::FileLoaded {
                path: PathBuf::from("/tmp/a.rs"),
                result: Ok(LoadedFile {
                    text: "hi\n".to_string(),
                    highlighted: Arc::new(vec![Line::from("hi")]),
                    rendered_markdown: None,
                }),
            },
        );
        assert_eq!(s.focus, Focus::Viewer);
        assert!(s.open_file.is_some());
        assert_eq!(s.open_file.as_ref().unwrap().highlighted.len(), 1);
    }

    #[test]
    fn viewer_down_key_scrolls() {
        let mut s = fixture();
        s.focus = Focus::Viewer;
        s.open_file = Some(OpenFile {
            path: PathBuf::from("/tmp/a.rs"),
            text: String::new(),
            highlighted: Arc::new(vec![Line::from("1"), Line::from("2"), Line::from("3")]),
            scroll: 0,
            error: None,
            diff_mode: false,
            diff: None,
            diff_error: None,
            edit: EditState::View,
            preview_mode: false,
            rendered_markdown: None,
        });
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        );
        assert_eq!(s.open_file.as_ref().unwrap().scroll, 1);
        // Clamped at last line:
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
        );
        assert_eq!(s.open_file.as_ref().unwrap().scroll, 2);
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        );
        assert_eq!(s.open_file.as_ref().unwrap().scroll, 2);
    }

    #[test]
    fn slash_opens_finder() {
        let mut s = fixture();
        update(&mut s, Msg::Key(plain('/')));
        assert!(matches!(s.overlay, Overlay::Finder(_)));
    }

    #[test]
    fn colon_opens_palette() {
        let mut s = fixture();
        update(&mut s, Msg::Key(plain(':')));
        assert!(matches!(s.overlay, Overlay::Palette(_)));
    }

    #[test]
    fn question_mark_opens_help() {
        let mut s = fixture();
        update(&mut s, Msg::Key(plain('?')));
        assert!(matches!(s.overlay, Overlay::Help));
    }

    #[test]
    fn esc_dismisses_overlay() {
        let mut s = fixture();
        s.overlay = Overlay::Help;
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        );
        assert!(matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn palette_enter_runs_quit_action() {
        let mut s = fixture();
        s.overlay = Overlay::Palette(PaletteState::new());
        if let Overlay::Palette(p) = &mut s.overlay {
            for c in "quit".chars() {
                p.push(c);
            }
        }
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        assert!(s.quit);
        assert!(matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn finder_enter_loads_selected_file() {
        let mut s = fixture();
        s.overlay = Overlay::Finder(FinderState::new(vec![finder::FinderItem {
            path: PathBuf::from("/tmp/foo.rs"),
            display: "foo.rs".to_string(),
        }]));
        let cmds = update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        assert!(
            matches!(cmds.as_slice(), [Cmd::LoadFile(p)] if p == &PathBuf::from("/tmp/foo.rs"))
        );
        assert!(matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn typing_in_finder_narrows_matches() {
        let mut s = fixture();
        let items = vec![
            finder::FinderItem {
                path: PathBuf::from("main.rs"),
                display: "main.rs".to_string(),
            },
            finder::FinderItem {
                path: PathBuf::from("README.md"),
                display: "README.md".to_string(),
            },
        ];
        s.overlay = Overlay::Finder(FinderState::new(items));
        update(&mut s, Msg::Key(plain('R')));
        update(&mut s, Msg::Key(plain('E')));
        if let Overlay::Finder(f) = &s.overlay {
            let top_display = &f.items[f.matches[0].index].display;
            assert!(
                top_display.contains("README"),
                "top match should be README.md, got {top_display}"
            );
        } else {
            panic!("overlay should still be finder");
        }
    }

    #[test]
    fn fs_change_under_git_sets_git_dirty() {
        let mut s = fixture();
        update(
            &mut s,
            Msg::FsChanged(vec![PathBuf::from("/tmp/.git/HEAD")]),
        );
        assert!(s.git_dirty, "changes under .git/ should mark git_dirty");
        // And still excluded from the change log.
        assert_eq!(s.changes.entries().len(), 0);
    }

    #[test]
    fn tick_emits_refresh_git_on_dirty_after_throttle() {
        let mut s = fixture();
        s.git_dirty = true;
        s.last_git_refresh = Instant::now() - Duration::from_secs(2);
        let cmds = update(&mut s, Msg::Tick);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::RefreshGit)));
    }

    #[test]
    fn tick_emits_refresh_git_on_5s_sweep_even_if_clean() {
        let mut s = fixture();
        s.git_dirty = false;
        s.last_git_refresh = Instant::now() - Duration::from_secs(6);
        let cmds = update(&mut s, Msg::Tick);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::RefreshGit)));
    }

    #[test]
    fn d_toggles_diff_and_emits_compute_first_time() {
        let mut s = fixture();
        s.open_file = Some(OpenFile {
            path: PathBuf::from("/tmp/a.rs"),
            text: String::new(),
            highlighted: Arc::new(Vec::new()),
            scroll: 0,
            error: None,
            diff_mode: false,
            diff: None,
            diff_error: None,
            edit: EditState::View,
            preview_mode: false,
            rendered_markdown: None,
        });
        // First press: enter diff mode + request computation.
        let cmds = update(&mut s, Msg::Key(plain('d')));
        assert!(s.open_file.as_ref().unwrap().diff_mode);
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Cmd::ComputeDiff(p) if p == &PathBuf::from("/tmp/a.rs")))
        );
        // Second press: exit diff mode, no new command.
        let cmds = update(&mut s, Msg::Key(plain('d')));
        assert!(!s.open_file.as_ref().unwrap().diff_mode);
        assert!(cmds.is_empty());
    }

    #[test]
    fn d_without_open_file_is_noop_with_status() {
        let mut s = fixture();
        let cmds = update(&mut s, Msg::Key(plain('d')));
        assert!(cmds.is_empty());
        assert!(s.status.is_some());
    }

    #[test]
    fn g_opens_git_status_when_snapshot_present() {
        let mut s = fixture();
        s.git_snapshot = Some(GitSnapshot::default());
        update(&mut s, Msg::Key(plain('g')));
        assert!(matches!(s.overlay, Overlay::GitStatus));
    }

    #[test]
    fn g_without_snapshot_shows_status() {
        let mut s = fixture();
        update(&mut s, Msg::Key(plain('g')));
        assert!(matches!(s.overlay, Overlay::None));
        assert!(s.status.is_some());
    }

    #[test]
    fn b_opens_worktree_switcher_with_current_preselected() {
        let mut s = fixture();
        let wts = vec![
            crate::git::WorktreeEntry {
                path: PathBuf::from("/a"),
                branch: None,
                is_current: false,
            },
            crate::git::WorktreeEntry {
                path: PathBuf::from("/b"),
                branch: None,
                is_current: true,
            },
        ];
        s.git_snapshot = Some(GitSnapshot {
            worktrees: wts,
            ..Default::default()
        });
        update(&mut s, Msg::Key(plain('b')));
        match &s.overlay {
            Overlay::WorktreeSwitcher(w) => assert_eq!(w.selected, 1),
            _ => panic!("expected WorktreeSwitcher overlay"),
        }
    }

    #[test]
    fn worktree_switcher_enter_on_current_is_noop_with_status() {
        let mut s = fixture();
        let wts = vec![crate::git::WorktreeEntry {
            path: PathBuf::from("/a"),
            branch: None,
            is_current: true,
        }];
        s.overlay = Overlay::WorktreeSwitcher(WorktreeSwitcherState::new(wts));
        let cmds = update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        assert!(cmds.is_empty());
        assert!(s.status.is_some());
        assert!(matches!(s.overlay, Overlay::None));
    }

    #[test]
    fn worktree_switcher_enter_on_other_emits_reroot() {
        let mut s = fixture();
        let wts = vec![
            crate::git::WorktreeEntry {
                path: PathBuf::from("/a"),
                branch: None,
                is_current: false,
            },
            crate::git::WorktreeEntry {
                path: PathBuf::from("/b"),
                branch: None,
                is_current: true,
            },
        ];
        s.overlay = Overlay::WorktreeSwitcher(WorktreeSwitcherState::new(wts));
        // Currently selected is /b (is_current). Move up to /a, then Enter.
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
        );
        let cmds = update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        );
        assert!(
            cmds.iter()
                .any(|c| matches!(c, Cmd::ReRoot(p) if p == &PathBuf::from("/a"))),
            "expected Cmd::ReRoot(/a)"
        );
    }

    #[test]
    fn m_toggles_preview_for_markdown_file() {
        let mut s = fixture();
        s.open_file = Some(OpenFile {
            path: PathBuf::from("/tmp/readme.md"),
            text: "# hi\n".to_string(),
            highlighted: Arc::new(Vec::new()),
            scroll: 5,
            error: None,
            diff_mode: false,
            diff: None,
            diff_error: None,
            edit: EditState::View,
            preview_mode: false,
            rendered_markdown: None,
        });
        update(&mut s, Msg::Key(plain('m')));
        assert!(s.open_file.as_ref().unwrap().preview_mode);
        // Entering preview resets scroll (line count differs from source).
        assert_eq!(s.open_file.as_ref().unwrap().scroll, 0);
        update(&mut s, Msg::Key(plain('m')));
        assert!(!s.open_file.as_ref().unwrap().preview_mode);
    }

    #[test]
    fn m_on_non_markdown_is_noop_with_status() {
        let mut s = fixture();
        s.open_file = Some(OpenFile {
            path: PathBuf::from("/tmp/foo.rs"),
            text: String::new(),
            highlighted: Arc::new(Vec::new()),
            scroll: 0,
            error: None,
            diff_mode: false,
            diff: None,
            diff_error: None,
            edit: EditState::View,
            preview_mode: false,
            rendered_markdown: None,
        });
        update(&mut s, Msg::Key(plain('m')));
        assert!(!s.open_file.as_ref().unwrap().preview_mode);
        assert!(s.status.is_some());
    }

    #[test]
    fn file_loaded_flashes_reloaded_when_already_open() {
        let mut s = fixture();
        s.open_file = Some(OpenFile {
            path: PathBuf::from("/tmp/a.rs"),
            text: String::new(),
            highlighted: Arc::new(Vec::new()),
            scroll: 0,
            error: None,
            diff_mode: false,
            diff: None,
            diff_error: None,
            edit: EditState::View,
            preview_mode: false,
            rendered_markdown: None,
        });
        update(
            &mut s,
            Msg::FileLoaded {
                path: PathBuf::from("/tmp/a.rs"),
                result: Ok(LoadedFile {
                    text: String::new(),
                    highlighted: Arc::new(vec![Line::from("x")]),
                    rendered_markdown: None,
                }),
            },
        );
        let (msg, _) = s.status.as_ref().expect("reload should produce a toast");
        assert!(msg.contains("reloaded"), "got {msg}");
    }

    #[test]
    fn file_loaded_first_time_does_not_flash_reloaded() {
        let mut s = fixture();
        assert!(s.open_file.is_none());
        update(
            &mut s,
            Msg::FileLoaded {
                path: PathBuf::from("/tmp/a.rs"),
                result: Ok(LoadedFile {
                    text: String::new(),
                    highlighted: Arc::new(vec![Line::from("x")]),
                    rendered_markdown: None,
                }),
            },
        );
        // Fresh open — should not produce the reload toast.
        assert!(
            s.status.is_none(),
            "first-time open should not flash reloaded"
        );
    }

    fn edit_test_path(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!("hitled_edit_{}_{}.rs", std::process::id(), suffix))
    }

    fn open_file_with_text(text: &str, path: PathBuf) -> OpenFile {
        OpenFile {
            path,
            text: text.to_string(),
            highlighted: Arc::new(Vec::new()),
            scroll: 0,
            error: None,
            diff_mode: false,
            diff: None,
            diff_error: None,
            edit: EditState::View,
            preview_mode: false,
            rendered_markdown: None,
        }
    }

    #[test]
    fn i_enters_edit_mode() {
        let mut s = fixture();
        s.open_file = Some(open_file_with_text(
            "hello\nworld\n",
            edit_test_path("i_enters"),
        ));
        update(&mut s, Msg::Key(plain('i')));
        match &s.open_file.as_ref().unwrap().edit {
            EditState::Edit(_) => {}
            _ => panic!("expected Edit state"),
        }
    }

    #[test]
    fn esc_exits_edit_mode_and_reports_discard_when_dirty() {
        let mut s = fixture();
        s.open_file = Some(open_file_with_text("hello\n", edit_test_path("esc_exits")));
        update(&mut s, Msg::Key(plain('i')));
        // Type something to dirty the buffer.
        if let EditState::Edit(b) = &mut s.open_file.as_mut().unwrap().edit {
            b.textarea.insert_char('x');
            assert!(b.is_dirty());
        }
        update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        );
        assert!(matches!(
            s.open_file.as_ref().unwrap().edit,
            EditState::View
        ));
        assert!(s.status.is_some(), "discard should produce a status toast");
    }

    #[test]
    fn ctrl_s_emits_savefile_with_current_content_and_arms_suppression() {
        let path = edit_test_path("ctrl_s");
        let mut s = fixture();
        s.open_file = Some(open_file_with_text("hi\n", path.clone()));
        update(&mut s, Msg::Key(plain('i')));
        if let EditState::Edit(b) = &mut s.open_file.as_mut().unwrap().edit {
            b.textarea.insert_char('!');
        }
        let cmds = update(
            &mut s,
            Msg::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
        );
        assert!(
            cmds.iter().any(|c| matches!(c, Cmd::SaveFile { .. })),
            "expected Cmd::SaveFile, got {cmds:?}",
        );
        assert_eq!(s.ignore_next_fs_change.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn fs_change_during_edit_transitions_to_conflict() {
        let p = edit_test_path("conflict");
        let mut s = fixture();
        s.open_file = Some(open_file_with_text("hello\n", p.clone()));
        update(&mut s, Msg::Key(plain('i')));
        std::fs::write(&p, b"external\n").unwrap();
        update(&mut s, Msg::FsChanged(vec![p.clone()]));
        assert!(matches!(
            s.open_file.as_ref().unwrap().edit,
            EditState::Conflict { .. }
        ));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn fs_change_after_self_save_is_suppressed() {
        let p = edit_test_path("self_save");
        let mut s = fixture();
        s.open_file = Some(open_file_with_text("hello\n", p.clone()));
        update(&mut s, Msg::Key(plain('i')));
        std::fs::write(&p, b"hello\n").unwrap();
        s.ignore_next_fs_change = Some(p.clone());
        update(&mut s, Msg::FsChanged(vec![p.clone()]));
        assert!(
            matches!(s.open_file.as_ref().unwrap().edit, EditState::Edit(_)),
            "own save must not trigger conflict"
        );
        assert!(s.ignore_next_fs_change.is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn conflict_k_keeps_mine_and_returns_to_edit() {
        let mut s = fixture();
        s.open_file = Some(open_file_with_text("hello\n", edit_test_path("conflict_k")));
        let buffer = EditBuffer::new("hello\n");
        s.open_file.as_mut().unwrap().edit = EditState::Conflict { buffer };
        update(&mut s, Msg::Key(plain('k')));
        assert!(matches!(
            s.open_file.as_ref().unwrap().edit,
            EditState::Edit(_)
        ));
    }

    #[test]
    fn conflict_t_emits_loadfile_and_returns_to_view() {
        let mut s = fixture();
        s.open_file = Some(open_file_with_text("hello\n", edit_test_path("conflict_t")));
        let buffer = EditBuffer::new("hello\n");
        s.open_file.as_mut().unwrap().edit = EditState::Conflict { buffer };
        let cmds = update(&mut s, Msg::Key(plain('t')));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::LoadFile(_))));
        assert!(matches!(
            s.open_file.as_ref().unwrap().edit,
            EditState::View
        ));
    }

    #[test]
    fn deleted_r_saves_buffer_and_returns_to_edit() {
        let mut s = fixture();
        s.open_file = Some(open_file_with_text("hello\n", edit_test_path("deleted_r")));
        let buffer = EditBuffer::new("hello\n");
        s.open_file.as_mut().unwrap().edit = EditState::Deleted { buffer };
        let cmds = update(&mut s, Msg::Key(plain('r')));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::SaveFile { .. })));
        assert!(matches!(
            s.open_file.as_ref().unwrap().edit,
            EditState::Edit(_)
        ));
    }

    #[test]
    fn deleted_c_closes_open_file() {
        let mut s = fixture();
        s.open_file = Some(open_file_with_text("hello\n", edit_test_path("deleted_c")));
        let buffer = EditBuffer::new("hello\n");
        s.open_file.as_mut().unwrap().edit = EditState::Deleted { buffer };
        update(&mut s, Msg::Key(plain('c')));
        assert!(s.open_file.is_none());
    }

    #[test]
    fn reroot_requested_sets_flag_for_run_session() {
        let mut s = fixture();
        update(&mut s, Msg::ReRootRequested(PathBuf::from("/new")));
        assert_eq!(s.reroot_to, Some(PathBuf::from("/new")));
    }

    #[test]
    fn status_expires_on_tick() {
        let mut s = fixture();
        s.status = Some(("hi".to_string(), Instant::now() - Duration::from_secs(10)));
        update(&mut s, Msg::Tick);
        assert!(s.status.is_none());
    }
}
