# Architecture

`chunk` is a Rust terminal UI built around four responsibilities:

- Load Git diffs and source snapshots.
- Parse unified diff text into typed model data.
- Maintain terminal interaction state.
- Render the current state with Ratatui.

## Module Map

- `src/main.rs`: CLI entry point. Parses `diff` and `pr` commands, then starts
  the app with a loaded `Changeset`.
- `src/model.rs`: Shared data model for changesets, files, hunks, lines, file
  status, staging state, and source snapshots.
- `src/git.rs`: Git boundary. Runs Git commands, builds worktree and branch
  diffs, detects staged/unstaged state, toggles file staging, and lazily loads
  source prefixes used by syntax highlighting.
- `src/patch.rs`: Unified diff parser. Converts Git patch text into
  `Changeset`, `DiffFile`, `DiffHunk`, and `DiffLine` values.
- `src/app.rs`: Application state and event loop. Owns selection, focus,
  scrolling, live reload, mouse handling, and terminal setup/teardown.
- `src/ui.rs`: Ratatui rendering. Lays out panes, renders the sidebar and diff,
  wraps lines, maps clicks to rows, and drives syntax highlighting for visible
  diffs.
- `src/syntax.rs`: Syntect adapter. Selects syntaxes from file paths and maps
  Syntect scopes onto the app's syntax palette.
- `src/theme.rs`: UI and syntax color palettes.

## Data Flow

1. `main` chooses `git::load_worktree_diff` or `git::load_pr_diff`.
2. `git` runs `git diff`, appends synthetic patches for untracked files in
   worktree mode, and passes the patch text to `patch::parse_unified_diff`.
3. `patch` returns a `Changeset` with files, hunks, line numbers, additions,
   deletions, and status metadata.
4. `app::run` enters alternate screen mode, enables raw input and mouse capture,
   then repeatedly draws and handles events.
5. `ui::draw` renders from `App`. When the selected diff needs rendering, it asks
   `App` to load source snapshots for that file.
6. `git::load_source_snapshots` loads only the source prefix needed before each
   hunk, capped by `MAX_SOURCE_CONTEXT_BYTES`.
7. `ui` uses those prefixes to advance Syntect state before highlighted diff
   lines, preserving syntax context for hunks that start mid-file.

## Worktree Mode

Worktree mode is interactive:

- It includes tracked changes and untracked files.
- It annotates each file as `Staged`, `Unstaged`, or `Mixed`.
- Pressing `Space` in the sidebar toggles staging for the selected file.
- A filesystem watcher reloads the worktree diff after relevant file or Git
  metadata changes, debounced by `WORKTREE_RELOAD_DEBOUNCE`.

Reload keeps the selected file by display path when possible. Diff scroll is
preserved only if the selected file still exists.

## PR Mode

PR mode is read-only. It resolves the base ref, computes `git merge-base
<base> HEAD`, and renders `git diff <merge-base> HEAD`.

Because the source is fixed Git refs, staging controls and live worktree reload
are disabled.

## Rendering Notes

The app stores the most recent rendered diff in `App::diff_lines_cache`. The
cache is invalidated when the selected file, content width, or syntax palette
changes.

Line wrapping happens after styling. The sidebar keeps a row-to-file-index map
for click handling because one file can wrap across multiple terminal rows.

## Design Boundaries

- `git.rs` is the only module that runs Git or reads source files.
- `patch.rs` does not run Git and does not know about terminal rendering.
- `app.rs` owns mutable UI state but delegates drawing to `ui.rs`.
- `ui.rs` formats model data but does not mutate Git state.
