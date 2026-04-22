/// Registry of actions reachable via the `:` command palette.
///
/// Kept intentionally small: the `?` help sheet lists bindings; the palette
/// exists for less-frequently-used commands and as a low-friction way to
/// discover functionality without memorizing keys.
#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub name: &'static str,
    pub description: &'static str,
    pub action: CommandAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandAction {
    ToggleSidebar,
    RefreshTree,
    CheckpointChanges,
    ShowHelp,
    GitStatus,
    Worktrees,
    Quit,
}

pub const COMMANDS: &[Command] = &[
    Command {
        name: "refresh",
        description: "Re-walk the file tree now",
        action: CommandAction::RefreshTree,
    },
    Command {
        name: "checkpoint",
        description: "Mark all changes seen",
        action: CommandAction::CheckpointChanges,
    },
    Command {
        name: "sidebar",
        description: "Show or hide the sidebar",
        action: CommandAction::ToggleSidebar,
    },
    Command {
        name: "git",
        description: "Show git status overlay",
        action: CommandAction::GitStatus,
    },
    Command {
        name: "worktrees",
        description: "Switch to another git worktree",
        action: CommandAction::Worktrees,
    },
    Command {
        name: "help",
        description: "Show the keybinding cheatsheet",
        action: CommandAction::ShowHelp,
    },
    Command {
        name: "quit",
        description: "Exit teep",
        action: CommandAction::Quit,
    },
];

/// Substring filter — the command list is small (~5), fuzzy scoring is overkill.
pub fn matches(query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..COMMANDS.len()).collect();
    }
    let q = query.to_lowercase();
    COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.name.to_lowercase().contains(&q) || c.description.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// State for the `:` command palette overlay.
pub struct PaletteState {
    pub query: String,
    pub matches: Vec<usize>,
    pub selected: usize,
}

impl PaletteState {
    pub fn new() -> Self {
        Self {
            matches: matches(""),
            query: String::new(),
            selected: 0,
        }
    }

    pub fn push(&mut self, c: char) {
        self.query.push(c);
        self.matches = matches(&self.query);
        self.selected = 0;
    }

    pub fn pop(&mut self) {
        if self.query.pop().is_some() {
            self.matches = matches(&self.query);
            self.selected = 0;
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

    pub fn selected_command(&self) -> Option<&'static Command> {
        self.matches
            .get(self.selected)
            .and_then(|&i| COMMANDS.get(i))
    }
}

impl Default for PaletteState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_returns_all() {
        assert_eq!(matches("").len(), COMMANDS.len());
    }

    #[test]
    fn query_filters_by_name() {
        let results = matches("refresh");
        assert_eq!(results.len(), 1);
        assert_eq!(COMMANDS[results[0]].name, "refresh");
    }

    #[test]
    fn query_filters_by_description() {
        let results = matches("cheatsheet");
        assert_eq!(results.len(), 1);
        assert_eq!(COMMANDS[results[0]].name, "help");
    }
}
