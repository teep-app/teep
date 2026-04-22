# HITLed

*Working name — to be renamed.*

> A live window into a codebase being edited by an AI coding agent.

HITLed is a small, opinionated Rust TUI that runs **alongside** your coding agent, not instead of it. The agent — Claude Code, Cursor CLI, Aider, whatever — edits your repo in one terminal pane; HITLed sits in the adjacent pane and shows you what the agent is actually doing to your files, in real time, with syntax highlighting, git context, and (soon) beautiful Markdown rendering.

It is not an IDE. It has no autocomplete, no LSP, no refactoring tools. It is a *viewer* with just enough editing to fix a typo.

---

## Why this exists

Agentic coding has a supervision problem. The agent scrolls tool calls and log lines in a terminal; your actual source files are somewhere else. The usual options for keeping an eye on things are all bad:

1. Trust the agent and check `git diff` at the end — risky, no context.
2. Jump into `nvim` / VS Code every few minutes — high friction, breaks your flow.
3. Keep a `watch git status` running — better, but ugly and not contextual.

HITLed is the fourth option: a persistent, always-on, auto-refreshing, git-aware file viewer designed for one specific workflow — *a human supervising a coding agent in a split pane*.

```mermaid
flowchart LR
    Dev([Developer])
    Agent["Coding Agent<br/>left pane"]
    FS[("Codebase<br/>+ git")]
    Hitled["HITLed<br/>right pane"]

    Dev      -->|prompt, review|    Agent
    Agent    -->|writes files|      FS
    FS       -.->|fs-watch events|  Hitled
    Hitled   -->|live view · diffs<br/>markdown · change log|  Dev
    Dev      -.->|occasional edit|  Hitled
    Hitled   -.->|save|             FS
```

Solid arrows are the main flow: you drive the agent, the agent edits the repo. Dotted arrows are HITLed's contribution: it tails the filesystem so you always know what just happened, and it lets you reach in and fix the occasional typo without context-switching out of the session.

---

## Status

Early. Under active development. The first public release is not done.

| Milestone | State | What works |
|---|---|---|
| M1 — skeleton | done | Terminal bootstrap, event loop, config, logging |
| M2 — tree + watch + viewer | done | File tree, live fs-watch, syntax-highlighted viewer, change log, `n`-cycle to next change |
| M3 — fuzzy finder + palette | next | `/` to open files, `:` for commands |
| M4 — git | planned | Branch, status, worktrees, inline diffs |
| M5 — edit + save | planned | Small edits; conflict banner on agent overwrite |
| M6 — Markdown render | planned | GFM with tables, task lists, code blocks |
| M7 — inline images | planned | Kitty graphics protocol (Ghostty-first) |
| M8 — mermaid | planned | Via `mmdc`, content-hash cached |
| M9 — polish + release | planned | Performance, docs, binary release |

The killer feature is **beautiful Markdown rendering with inline images and mermaid diagrams, in the terminal, right next to the agent writing the Markdown.** Most of HITLed exists to get to M6-M8 with the right foundation underneath.

---

## Install and run

Requires Rust 1.93+.

```sh
git clone <this repo>
cd hitled
cargo install --path .
```

Then, in your repo:

```sh
hitled .          # open the current directory
hitled path/to/repo
```

Put it in the right-hand pane of whatever terminal multiplexer you're using (tmux, Ghostty's split panes, WezTerm, Zellij, etc.) and your coding agent in the left.

**Primary target terminal is Ghostty** because the Markdown feature will lean on the Kitty graphics protocol for inline images. Everything *except* inline images works in any crossterm-compatible terminal.

**Optional runtime dep**: `mmdc` (mermaid-cli, via `brew install mermaid-cli` or `npm i -g @mermaid-js/mermaid-cli`). Without it, mermaid blocks render as placeholders.

---

## Keybindings (current)

Modeless navigation, with a single modal concession for edit mode (M5, not yet).

| Key | Action |
|---|---|
| `↑` `↓` | Move tree selection / scroll viewer (depending on focus) |
| `→` `←` | Expand / collapse dir in tree |
| `Enter` / `o` | Open file (or toggle dir) |
| `n` / `N` | Next / prev agent-changed file |
| `u` | Mark all changes seen (checkpoint) |
| `Tab` | Switch focus: tree ↔ viewer |
| `PgUp` / `PgDn` / `Home` / `End` | Scroll viewer |
| `Ctrl-B` | Toggle sidebar |
| `Ctrl-C Ctrl-C` | Quit |

Footer always shows a context-aware subset of relevant bindings. Full keymap lands with M3's command palette.

---

## Design principles

1. **The agent is the real IDE.** HITLed never competes with it. No LSP, no autocomplete, no multi-cursor, no language-specific refactoring.
2. **Viewer-first.** Editing is possible but not the point. Cursor-blinking and raw-mode semantics are kept simple.
3. **Fast startup.** Should launch in under 150 ms on a warm cache. If HITLed feels heavy, it fails its job.
4. **Live by default.** File tree, change log, diffs, and Markdown preview all update as the agent writes. No manual refresh keys.
5. **Beautiful Markdown is the feature.** Most agentic work revolves around `.md` files — plans, specs, CLAUDE.md, READMEs. HITLed exists to make those feel *good* in a terminal.
6. **Graceful degradation.** If your terminal can't do the Kitty graphics protocol, images become placeholders and everything else still works.
7. **Single small binary.** No chromium. No JVM. No Node. One `cargo install` and you're done. (External `mmdc` is the one optional exception.)

---

## Not for you if

- You want a full editor. Use Neovim, Helix, or VS Code.
- You don't work with AI coding agents. HITLed's workflow assumes you do.
- You live entirely in an IDE's integrated terminal and never split panes.

---

## Architecture in one paragraph

Elm-style. A single `AppState`, a single `Msg` enum, a pure `update(state, msg) -> (state, Vec<Cmd>)` function. Terminal events, `notify` fs-watch events, and async job completions all funnel into one `mpsc::UnboundedReceiver<Msg>`. A `Runtime` executes `Cmd`s (file reads, syntax highlighting, later git and mermaid work) on spawned Tokio tasks, posting results back as `Msg`s. Rendering is a pure function of state, run on a 250 ms tick plus whenever a `Msg` is processed. The state layer has no ratatui dependency for its core logic, which makes `update` unit-testable without a terminal.

See [the design plan](./docs/plan.md) for the long version. *(coming with M6; source of truth currently lives in the contributor's plan file)*

---

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache 2.0](./LICENSE-APACHE), at your option.

Contributions welcome once v0.1 tags. Until then, issues and design feedback are more useful than patches.
