# Teep

> *Your private telepath for the agent writing your code.*

Teep sits in the pane next to your coding agent and reads the room. Every file the agent touches, every change it slips in, every edit that needs a second pair of eyes — Teep is already watching. Syntax-highlighted, git-aware, beautifully rendered Markdown, live the moment the agent writes it.

*You drive the agent. Teep watches the agent.*

A terminal viewer, not an editor. Fast to start, quiet when nothing's happening, impossible to miss when something is. Built in Rust. Ghostty-first. One binary plus the `git` you already have. Press `n` to jump to whatever the agent just did. Press `u` when you've seen enough. That's the loop.

---

## Why

Agentic coding has a supervision problem. The agent scrolls tool calls and log lines in one pane; the actual source files are somewhere else. The usual options for keeping an eye on things are all bad:

1. Trust the agent and check `git diff` at the end — risky, no context.
2. Jump into `nvim` / VS Code every few minutes — high friction, breaks your flow.
3. Keep a `watch git status` running — better, but ugly and uncontextual.

Teep is the fourth option: a persistent, always-on, auto-refreshing, git-aware file viewer designed for one specific workflow — *a human supervising a coding agent in a split pane*.

```mermaid
flowchart LR
    Dev([Developer])
    Agent["Coding Agent<br/>left pane"]
    FS[("Codebase<br/>+ git")]
    Teep["Teep<br/>right pane"]

    Dev -->|"prompt, review"| Agent
    Agent -->|"writes files"| FS
    FS -.->|"fs-watch events"| Teep
    Teep -->|"live view · diffs<br/>markdown · change log"| Dev
    Dev -.->|"occasional edit"| Teep
    Teep -.->|"save"| FS
```

Solid arrows are the main flow: you drive the agent, the agent edits the repo. Dotted arrows are Teep's contribution: it tails the filesystem so you always know what just happened, and it lets you reach in and fix the occasional typo without context-switching out of the session.

---

## Status

Early. Under active development. MVP in sight.

| Milestone | State | What works |
|---|---|---|
| M1 — skeleton | done | Terminal bootstrap, event loop, config, logging |
| M2 — tree + watch + viewer | done | File tree, live fs-watch, syntax-highlighted viewer, change log, `n`-cycle to next change |
| M3 — fuzzy finder + palette | done | `/` fuzzy file finder, `:` command palette, `?` help |
| M4 — git | done | Branch, status, worktrees, inline diffs |
| M5 — edit + save | done | Small edits; conflict banner on agent overwrite |
| M6 — Markdown render | done | GFM with tables, task lists, code blocks, styled inlines |
| M6.5 — live preview (Obsidian-style) | done | Reveal-on-cursor: current block raw, everything else cooked |
| M7 — inline images | done | Kitty / iTerm2 / Sixel / halfblocks fallback |
| M7.1 — inline markdown images | done | `![](path)` renders in Live Preview, reveals source on cursor |
| M8 — mermaid | done | Via `mmdc`, content-hash cached; placeholder when mmdc missing |
| M9 — polish + release | in progress | Performance, docs, binary release |

The killer feature is **beautiful Markdown rendering with inline images and mermaid diagrams, in the terminal, right next to the agent writing the Markdown.** Most of Teep exists to get there with the right foundation underneath.

---

## Install and run

Requires Rust 1.93+.

```sh
# Public install via Homebrew (once v0.1.0 ships):
brew install teep-app/teep/teep

# Or build from source (requires access to teep-app/source):
git clone https://github.com/teep-app/source teep
cd teep
cargo install --path .
```

Then, in your repo:

```sh
teep .           # open the current directory
teep path/to/repo
```

Put it in the right-hand pane of whatever terminal multiplexer you're using (tmux, Ghostty's split panes, WezTerm, Zellij, etc.) and your coding agent in the left.

**Primary target terminal is Ghostty** because the Markdown feature leans on the Kitty graphics protocol for inline images. Everything *except* high-quality inline images works in any crossterm-compatible terminal (fallback to halfblocks).

**Optional runtime dep**: `mmdc` (mermaid-cli, via `brew install mermaid-cli` or `npm i -g @mermaid-js/mermaid-cli`). Without it, mermaid blocks render as placeholders.

**Inside tmux**: run `tmux set -g allow-passthrough on` so the graphics protocol escape sequences reach the terminal.

---

## Keybindings

Modeless navigation with a single modal concession for edit mode.

| Key | Action |
|---|---|
| `↑` `↓` | Move tree selection / scroll viewer (depending on focus) |
| `→` `←` | Expand / collapse dir in tree |
| `Enter` / `o` | Open file (or toggle dir) |
| `n` / `N` | Next / prev agent-changed file |
| `u` | Mark all changes seen (checkpoint) |
| `r` | Force tree refresh |
| `Tab` | Switch focus: tree ↔ viewer |
| `/` or `Ctrl-P` | Fuzzy open |
| `:` | Command palette |
| `?` | Help overlay |
| `d` | Toggle inline diff vs HEAD |
| `m` | Toggle Markdown Live Preview (on `.md` files) |
| `g` | Git status overlay |
| `b` | Branch / worktree switcher |
| `i` / `e` | Enter edit mode (`Esc` exits) |
| `Ctrl-S` | Save |
| `Ctrl-B` | Toggle sidebar |
| `PgUp` / `PgDn` / `Home` / `End` | Scroll viewer |
| `Ctrl-C Ctrl-C` | Quit |

Footer always shows a context-aware subset. Full list in the `?` overlay.

---

## Design principles

1. **The agent is the real IDE.** Teep never competes with it. No LSP, no autocomplete, no multi-cursor, no language-specific refactoring.
2. **Viewer-first.** Editing is possible but not the point.
3. **Fast startup.** Launch in under 150 ms on a warm cache. If Teep feels heavy, it fails its job.
4. **Live by default.** File tree, change log, diffs, and Markdown preview all update as the agent writes. No manual refresh keys.
5. **Beautiful Markdown is the feature.** Most agentic work revolves around `.md` files — plans, specs, CLAUDE.md, READMEs. Teep exists to make those feel *good* in a terminal.
6. **Graceful degradation.** If your terminal can't do the Kitty graphics protocol, images become halfblocks or placeholders and everything else still works.
7. **Single small binary.** No chromium. No JVM. No Node. One `cargo install` and you're done. (External `mmdc` is the one optional exception.)

---

## Not for you if

- You want a full editor. Use Neovim, Helix, or VS Code.
- You don't work with AI coding agents. Teep's workflow assumes you do.
- You live entirely in an IDE's integrated terminal and never split panes.

---

## Architecture in one paragraph

Elm-style. A single `AppState`, a single `Msg` enum, a pure `update(state, msg) -> (state, Vec<Cmd>)` function. Terminal events, `notify` fs-watch events, and async job completions all funnel into one `mpsc::UnboundedReceiver<Msg>`. A `Runtime` executes `Cmd`s (file reads, syntax highlighting, git snapshot, diff, image decode) on spawned Tokio tasks, posting results back as `Msg`s. Rendering is a pure function of state, run on a 250 ms tick plus whenever a `Msg` is processed. The state layer has no ratatui dependency for its core logic, which makes `update` unit-testable without a terminal.

---

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache 2.0](./LICENSE-APACHE), at your option.

Contributions welcome once v0.1 tags. Until then, issues and design feedback are more useful than patches.
