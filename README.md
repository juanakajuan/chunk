# chunk

`chunk` is a terminal diff reviewer for Git repositories. It shows a tree file
list beside a syntax-highlighted unified diff, with live worktree refresh,
file/hunk staging, guarded discard actions, mouse support, and optional OpenCode
AI actions.

## Requirements

- Rust 2024 toolchain
- Git
- [OpenCode](https://opencode.ai/) for Ask AI actions
- A [Nerd Font](https://www.nerdfonts.com/) for file/status glyphs

## Usage

Run inside a Git worktree:

```sh
cargo run -- diff       # live worktree review; also the default command
cargo run -- pr [base]  # branch review from merge-base(base, HEAD) to HEAD
cargo run -- diff --json
```

`diff` reviews staged, unstaged, mixed, and untracked changes against `HEAD` and
live-refreshes as the worktree changes.

`pr` is read-only: no live refresh, staging, or discard. Without a base ref it
tries `origin/HEAD`, then `main`, then `master`.

`diff --json` prints one worktree snapshot for external frontends instead of
starting the terminal UI. The output includes `title`, `source_label`, and
`files`; each file includes paths, status, stage, line counts, binary state, and
hunks with stable old/new ranges and diff lines.

## Controls

These are the default keys. Most single-character actions can be remapped in
`[keybinds]`; special keys and `Ctrl-*` combos stay fixed.

| Keys | Action |
| ---- | ------ |
| `j` / `k` | Move in the file list or scroll the diff |
| `Tab`, `Left`, `Right`, `Enter` | Change focus between files and diff |
| `f` | Toggle the file list |
| `/`, `n`, `N` | Search the selected file; jump next/previous match or hunk |
| `PageDown` / `Ctrl-d`, `PageUp` / `Ctrl-u` | Page the diff |
| `g` / `Home`, `G` / `End` | Jump to top or bottom |
| `?` | Toggle in-app help |
| `a`, `x`, `u` | Ask AI, explain selection, or summarize unpublished changes |
| `y`, `Y` | Copy focused path/hunk or selected file diff |
| `r` | Toggle reviewed state for the selected file |
| `Space` | Stage/unstage the selected file or hunk in `diff` mode |
| `d` | Discard unstaged file/folder/hunk changes after confirmation in `diff` mode |
| `e` | Open the selected file in `$EDITOR` near its first changed line |
| Custom command keys | Run configured shell commands from the Git root |
| `q` / `Ctrl-c` | Quit |

Mouse hover changes focus. Click files, folders, and hunks to select or expand
them; use the wheel to scroll. Drag visible text to copy it.

Command output and Ask AI panes use the same scroll keys; close them with `Esc`
or `q`. Ask AI answers can be copied with `y`.

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

Supported themes are `gruvbox` (default) and `github-dark`.

Keybind values must be single characters; use a literal space for `Space`.
Unknown actions, invalid keys, conflicting keys, and duplicate custom command
keys are rejected at startup. Remapping a built-in frees its default key for a
custom command.

Custom commands run from the Git root through the user shell. After they finish,
`chunk` shows stdout, stderr, and exit status, then reloads the review source.

## Development

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```
