# chunk

`chunk` is a minimal terminal diff reviewer for Git repositories. It renders a
file list and a syntax-highlighted unified diff in the terminal, with keyboard
and mouse navigation.

## Usage

Run from inside a Git worktree:

```sh
cargo run -- diff
```

`diff` reviews the current working tree against `HEAD`, including untracked
files. This is also the default command:

```sh
cargo run
```

Review the current branch like a pull request:

```sh
cargo run -- pr
cargo run -- pr main
```

When no base is passed, `pr` tries `origin/HEAD`, then `main`, then `master`.

## Controls

- `j` / `k`: move down or up in the focused pane
- `Tab`: switch focus between file list and diff
- `Left`: focus file list
- `Right` / `Enter`: focus diff
- `PageDown` / `Ctrl-d`: scroll diff down one page
- `PageUp` / `Ctrl-u`: scroll diff up one page
- `g` / `Home`: jump to top of diff
- `G` / `End`: jump to bottom of diff
- `Space`: stage or unstage the selected file in `diff` mode
- `q` / `Esc`: quit

Mouse hover changes focus. Click a file to select it. Wheel scrolling moves
through files in the sidebar and scrolls the diff in the diff pane.

## Themes

The default theme is `matte_box`. Set `CHUNK_THEME=github-dark` at compile time
to use the GitHub dark palette:

```sh
CHUNK_THEME=github-dark cargo run
```

## Development

```sh
cargo test
```

See [docs/architecture.md](docs/architecture.md) for the code map and data flow.
