# chunk

`chunk` is a compact terminal diff reviewer for Git repositories. It renders a
file list and a syntax-highlighted unified diff in the terminal, with keyboard
and mouse navigation, live worktree refresh, file/hunk staging, and guarded
worktree discard actions.

## Requirements

- Rust 2024 toolchain
- Git
- [OpenCode](https://opencode.ai/) for the Ask AI action
- A [Nerd Font](https://www.nerdfonts.com/) for the file-type, status, and
  staging glyphs in the sidebar and bottom bar

## Usage

Run from inside a Git worktree:

```sh
cargo run -- diff
```

`diff` reviews the current working tree against `HEAD`, including staged,
unstaged, mixed, and untracked files. It live-refreshes as the worktree changes.
This is also the default command:

```sh
cargo run
```

Review the current branch like a pull request:

```sh
cargo run -- pr
cargo run -- pr main
```

When no base is passed, `pr` tries `origin/HEAD`, then `main`, then `master`.
PR mode compares the merge-base of the base ref and `HEAD` against `HEAD`.
It does not live-refresh, stage files, or discard worktree changes.

## Controls

The keys below are the defaults and can be remapped in `[keybinds]` (except
for the special keys and `Ctrl-*` combos noted inline).

- `j` / `k`: move down or up in the focused pane; in the file list this changes
  files, in the diff pane this scrolls by one row
- `Tab`: switch focus between file list and diff
- `f`: show or hide the file list
- `Left`: focus file list
- `Right` / `Enter`: focus diff
- `PageDown` / `Ctrl-d`: scroll diff down one page
- `PageUp` / `Ctrl-u`: scroll diff up one page
- `/`: open a literal search prompt for the selected file; `Enter` applies the
  query and `Esc` cancels or clears search
- `n` / `N`: with search active, jump to the next or previous match; otherwise
  select and jump to the next or previous hunk in the selected file
- `g` / `Home`: jump to top of diff
- `G` / `End`: jump to bottom of diff
- `?`: show or hide the in-app keymap help; the overlay also closes with
  `Esc` or `q`
- `a`: ask OpenCode a read-only question about the selected file from the file
  list, or the focused hunk from the diff pane; `Enter` submits, `Esc` cancels
  the prompt or running request
- `x`: ask OpenCode to explain the selected file from the file list, or the
  focused hunk from the diff pane, using the preset review-oriented prompt
- `y`: copy the selected file path from the file list, or the selected hunk
  diff from the diff pane
- `Y`: copy the selected file diff from the diff pane
- `r`: toggle reviewed state for the selected file; reviewed files are dimmed
  and marked with a check in the file list, and the state survives reloads as
  long as the file path stays the same
- `Space`: in `diff` mode, stage or unstage the selected file when the file list
  is focused, or the selected hunk when the diff pane is focused
- `d`: in `diff` mode, discard unstaged worktree changes for the selected file
  or hunk after confirmation; untracked files can be discarded from the file list
- `e`: open the selected file in `$EDITOR` near the first changed line in `diff`
  mode
- Custom command keys from config: run the configured shell command from the Git
  root, then show stdout, stderr, and exit status in a command pane
- `q` / `Ctrl-c`: quit

Mouse hover changes focus. Click a file to select it, or click a hunk in the
diff pane to select it. Wheel scrolling moves through files in the sidebar and
scrolls the diff in the diff pane. Drag visible text to copy the selected text
to the clipboard.

In the command output pane, use `j` / `k` or the mouse wheel to scroll,
`PageDown` / `PageUp` to page, `g` / `G` to jump, and `Esc` / `q` to return to
the diff.

In the Ask AI answer pane, use the same scroll and close keys, and `y` to copy
the answer text. The OpenCode process receives the Git root, focused file or
hunk, selected visible text when present, read-only repository permissions, and
web fetch/search access. Answers render Markdown for common formatting such as
headings, lists, links, quotes, and code blocks.

## Configuration

`chunk` reads `${XDG_CONFIG_HOME:-$HOME/.config}/chunk/config.toml` when it
exists.

```toml
theme = "github-dark"

[keybinds]
quit = "Q"
discard = "D"

[[commands]]
key = "C"
label = "commit and push"
command = "git add . && com && git push"
```

The `theme` setting is optional. Supported values are `gruvbox` (default) and
`github-dark`.

### Built-in keybinds

The `[keybinds]` table remaps selected built-in actions. Each value is a single
character (use a literal space for `Space`). Unknown action names, invalid
keys, and keys shared by two actions are rejected at startup. Special keys
(`Tab`, `Enter`, `Esc`, arrows, `PageUp`/`PageDown`, `Home`/`End`) and
`Ctrl-*` combos stay fixed and are not listed here.

| Action           | Default | Action           | Default |
| ---------------- | ------- | ---------------- | ------- |
| `quit`           | `q`     | `toggle_staging` | `Space` |
| `help`           | `?`     | `discard`        | `d`     |
| `toggle_files`   | `f`     | `editor`         | `e`     |
| `search`         | `/`     | `ask_ai`         | `a`     |
| `move_down`      | `j`     | `explain_code`   | `x`     |
| `move_up`        | `k`     | `copy_focused`   | `y`     |
| `next_match`     | `n`     | `copy_file_diff` | `Y`     |
| `prev_match`     | `N`     | `toggle_reviewed`| `r`     |
| `top`            | `g`     |                  |         |
| `bottom`         | `G`     |                  |         |

Remapping a built-in frees its default key for custom commands, and the help
overlay, keymap bar, and overlay close keys all follow the configured keys.

### Custom commands

Custom command keys are single characters. Custom command keys conflicting
with any configured built-in keybind, and duplicate custom keys, are rejected
at startup. Commands run from the Git repository root through the user shell,
and `chunk` reloads the review source after completion.

## Themes

The default theme is Gruvbox (dark, hard contrast). Set `theme = "github-dark"`
in `${XDG_CONFIG_HOME:-$HOME/.config}/chunk/config.toml` to use the GitHub dark
palette.

## Development

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Code map:

- `src/main.rs`: CLI parsing and review source selection
- `src/config.rs`: user config loading and validation
- `src/keybind.rs`: configurable built-in keybind map and validation
- `src/ask_ai.rs`: read-only OpenCode request and prompt boundary
- `src/custom_command.rs`: configured shell command bindings and execution
- `src/editor.rs`: external editor command resolution
- `src/review_source.rs`: worktree vs PR behavior, reloads, mutation capability
- `src/git.rs`: Git command boundary and source snapshot loading
- `src/patch.rs`: unified diff parser
- `src/model.rs`: parsed diff data structures
- `src/app.rs`: selection, focus, scroll, reload, and staging session state
- `src/runtime.rs`: terminal setup, event loop, mouse capture, live watcher
- `src/ui.rs`: Ratatui layout and widget drawing
- `src/rows.rs` and `src/rows/`: rendered sidebar, diff, status, and keybind rows
- `src/viewport.rs`: viewport geometry, scroll clamping, render caches
- `src/syntax.rs` and `src/theme.rs`: syntax adapter and palettes
