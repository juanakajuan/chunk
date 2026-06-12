# chunk

`chunk` is a compact terminal diff reviewer for Git repositories. It renders a
file list and a syntax-highlighted unified diff in the terminal, with keyboard
and mouse navigation, live worktree refresh, and file/hunk staging.

## Requirements

- Rust 2024 toolchain
- Git
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
It does not live-refresh or stage files.

## Controls

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
- `Space`: in `diff` mode, stage or unstage the selected file when the file list
  is focused, or the selected hunk when the diff pane is focused
- `e`: open the selected file in `$EDITOR` near the first changed line in `diff`
  mode
- `q` / `Ctrl-c`: quit

Mouse hover changes focus. Click a file to select it, or click a hunk in the
diff pane to select it. Wheel scrolling moves through files in the sidebar and
scrolls the diff in the diff pane.

## Themes

The default theme is Gruvbox (dark, hard contrast). Set `CHUNK_THEME=github-dark`
at compile time to use the GitHub dark palette instead:

```sh
CHUNK_THEME=github-dark cargo run
```

## Development

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Code map:

- `src/main.rs`: CLI parsing and review source selection
- `src/editor.rs`: external editor command resolution
- `src/review_source.rs`: worktree vs PR behavior, reloads, staging capability
- `src/git.rs`: Git command boundary and source snapshot loading
- `src/patch.rs`: unified diff parser
- `src/model.rs`: parsed diff data structures
- `src/app.rs`: selection, focus, scroll, reload, and staging session state
- `src/runtime.rs`: terminal setup, event loop, mouse capture, live watcher
- `src/ui.rs`: Ratatui layout and widget drawing
- `src/rows.rs` and `src/rows/`: rendered sidebar, diff, status, and keybind rows
- `src/viewport.rs`: viewport geometry, scroll clamping, render caches
- `src/syntax.rs` and `src/theme.rs`: syntax adapter and palettes
