use std::{
    io::Stdout,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect, text::Line};

use crate::{
    changes::ChangeLog,
    commands::{CommandAction, PaletteState},
    config::Config,
    event::EventLoop,
    finder::{self, FinderState},
    fs_watch,
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
}

/// Modal overlays that steal keyboard focus. Triggered by `/`, `:`, `?`.
#[derive(Default)]
pub enum Overlay {
    #[default]
    None,
    Finder(FinderState),
    Palette(PaletteState),
    Help,
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
    #[allow(dead_code)] // full text retained for future edit mode / diff in M4/M5
    pub text: String,
    pub highlighted: Arc<Vec<Line<'static>>>,
    pub scroll: usize,
    pub error: Option<String>,
}

pub struct LoadedFile {
    pub text: String,
    pub highlighted: Arc<Vec<Line<'static>>>,
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
    TreeRebuilt(tree::Node),
    Tick,
}

pub enum Cmd {
    LoadFile(PathBuf),
    RebuildTree,
}

const CTRL_C_QUIT_WINDOW: Duration = Duration::from_millis(1000);
const STATUS_LIFETIME: Duration = Duration::from_secs(2);
const PAGE_SCROLL: usize = 20;
const TREE_REBUILD_THROTTLE: Duration = Duration::from_millis(500);

pub fn update(state: &mut AppState, msg: Msg) -> Vec<Cmd> {
    let mut cmds = Vec::new();
    match msg {
        Msg::Key(key) => handle_key(state, key, &mut cmds),
        Msg::Mouse(ev) => handle_mouse(state, ev, &mut cmds),
        Msg::FsChanged(paths) => handle_fs_changed(state, paths, &mut cmds),
        Msg::FileLoaded { path, result } => handle_file_loaded(state, path, result),
        Msg::TreeRebuilt(node) => state.tree.graft(node),
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
        _ => match state.focus {
            Focus::Tree => handle_tree_key(state, key, cmds),
            Focus::Viewer => handle_viewer_key(state, key),
        },
    }
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
            Overlay::Help | Overlay::None => {}
        },
        KeyCode::Down => match &mut state.overlay {
            Overlay::Finder(f) => f.move_down(),
            Overlay::Palette(p) => p.move_down(),
            Overlay::Help | Overlay::None => {}
        },
        KeyCode::Backspace => match &mut state.overlay {
            Overlay::Finder(f) => f.pop(),
            Overlay::Palette(p) => p.pop(),
            Overlay::Help | Overlay::None => {}
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
            Overlay::Help | Overlay::None => {}
        },
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            match &mut state.overlay {
                Overlay::Finder(f) => f.push(c),
                Overlay::Palette(p) => p.push(c),
                Overlay::Help => state.overlay = Overlay::None,
                Overlay::None => {}
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

fn handle_fs_changed(state: &mut AppState, paths: Vec<PathBuf>, _cmds: &mut Vec<Cmd>) {
    let mut touched_anything = false;
    for path in paths {
        if is_noise(&path) {
            continue;
        }
        touched_anything = true;
        match path.metadata() {
            Ok(m) if m.is_dir() => continue,
            Ok(_) => {}
            Err(_) => continue, // nonexistent; is_noise already filters most of these
        }
        state.changes.record(path.clone());
        if let Some(open) = &state.open_file
            && open.path == path
        {
            _cmds.push(Cmd::LoadFile(path));
        }
    }
    if touched_anything {
        state.tree_dirty = true;
    }
}

/// Returns true for fs events we should not record or act on: dotfiles,
/// `.git/*`, common editor atomic-rename temp files, paths that no longer
/// exist. Matches the filtering behavior of `tree::build_node`
/// (which uses `WalkBuilder::hidden(true)` + `.git_ignore(true)`).
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
    if !path.exists() {
        return true;
    }
    false
}

fn handle_file_loaded(state: &mut AppState, path: PathBuf, result: Result<LoadedFile, String>) {
    state.changes.mark_seen(&path);
    match result {
        Ok(loaded) => {
            let preserve_scroll = state
                .open_file
                .as_ref()
                .filter(|f| f.path == path)
                .map(|f| f.scroll.min(loaded.highlighted.len().saturating_sub(1)));
            state.open_file = Some(OpenFile {
                path,
                text: loaded.text,
                highlighted: loaded.highlighted,
                scroll: preserve_scroll.unwrap_or(0),
                error: None,
            });
            state.focus = Focus::Viewer;
        }
        Err(e) => {
            let msg = format!("failed: {e}");
            state.open_file = Some(OpenFile {
                path,
                text: String::new(),
                highlighted: Arc::new(Vec::new()),
                scroll: 0,
                error: Some(msg.clone()),
            });
            set_status(state, msg);
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
}

fn set_status(state: &mut AppState, msg: String) {
    state.status = Some((msg, Instant::now()));
}

pub async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    root: PathBuf,
    config: Config,
) -> Result<()> {
    let tree = Tree::build(&root)?;
    let mut state = AppState {
        root: root.clone(),
        config,
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
    };

    // Pre-warm the syntect OnceLocks so the first real file open isn't a
    // noticeable pause. Using a plain OS thread since the work is pure CPU
    // with no tokio affinity and its completion isn't needed by anyone.
    std::thread::spawn(|| {
        crate::syntax::highlight_file("", std::path::Path::new("warmup.txt"));
    });

    let mut events = EventLoop::new();
    let runtime = Runtime::new(events.sender(), root.clone());
    let _fs_watcher = fs_watch::spawn(root, events.sender())?;

    while !state.quit {
        terminal.draw(|f| crate::ui::view(&mut state, f))?;
        let Some(msg) = events.next().await else {
            break;
        };
        let cmds = update(&mut state, msg);
        for cmd in cmds {
            runtime.execute(cmd);
        }
    }
    Ok(())
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
        assert!(
            is_noise(Path::new("/tmp/definitely_does_not_exist_hitled_test.rs")),
            "nonexistent paths are noise"
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
    fn status_expires_on_tick() {
        let mut s = fixture();
        s.status = Some(("hi".to_string(), Instant::now() - Duration::from_secs(10)));
        update(&mut s, Msg::Tick);
        assert!(s.status.is_none());
    }
}
