use super::*;
use crate::ask_ai::{AiHarness, AiSettings, AskAiResult, AskAiReviewMode};
use crate::custom_command::CustomCommandResult;
use crate::diff_render::RenderedDiffLines;
use crate::keybind::{BuiltinAction, BuiltinKey, KeybindMap};
use crate::model::{
    CommitDetails, CommitSummary, DiffHunk, DiffLine, DiffLineKind, FileStatus, LocalBranch,
    RepositoryOverview, SourceSnapshot,
};
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::text::Line;

#[test]
fn diff_scroll_bounds_use_rendered_rows_when_available() {
    let mut app = app_with(changeset_with_one_file());
    app.selected_diff.begin_test_frame(Rect::default(), 3);
    app.selected_diff.set_scroll(99);
    app.selected_diff.cache_test_diff_lines(
        0,
        RenderedDiffLines::new(
            "0".to_string(),
            24,
            Theme::github_dark().syntax,
            true,
            vec![Line::raw("row"); 8],
            true,
        ),
    );

    app.ensure_scroll_bounds();

    assert_eq!(app.selected_diff.scroll(), 5);
}

#[test]
fn reload_preserves_selected_file_and_scroll_by_path() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    app.files_panel
        .set_selected_file(1, app.changeset.files.len());
    app.selected_diff.begin_test_frame(Rect::default(), 3);
    app.selected_diff.set_scroll(4);

    app.apply_reloaded_changeset(changeset_with_paths(["b.txt", "a.txt"]), true);

    assert_selected_file_path(&app, "b.txt");
    assert_eq!(app.selected_file_index(), 0);
    assert_eq!(app.selected_diff.scroll(), 4);
}

#[test]
fn reload_preserves_selected_hunk_by_coordinates() {
    let mut app = app_with(changeset_with_two_hunk_file());
    app.selected_diff.set_selected_hunk_index(Some(1));

    app.apply_reloaded_changeset(changeset_with_two_hunk_file(), true);

    assert_eq!(app.selected_diff.selected_hunk_index(), Some(1));
}

#[test]
fn reload_clamps_scroll_when_selected_file_shrinks() {
    let mut app = app_with(changeset_with_paths(["sample.txt"]));
    app.selected_diff.begin_test_frame(Rect::default(), 3);
    app.selected_diff.set_scroll(99);

    app.apply_reloaded_changeset(changeset_with_short_file("sample.txt"), true);

    assert_selected_file_path(&app, "sample.txt");
    assert_eq!(app.selected_diff.scroll(), 0);
}

#[test]
fn reload_resets_selection_and_scroll_when_selected_file_disappears() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    app.files_panel
        .set_selected_file(1, app.changeset.files.len());
    app.selected_diff.set_scroll(4);

    app.apply_reloaded_changeset(changeset_with_paths(["a.txt"]), true);

    assert_selected_file_path(&app, "a.txt");
    assert_eq!(app.selected_diff.scroll(), 0);
}

#[test]
fn hiding_files_panel_moves_focus_to_diff() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    app.files_panel
        .set_selected_file(1, app.changeset.files.len());
    app.files_panel.set_scroll(1);
    app.selected_diff.set_scroll(3);

    press_key(&mut app, KeyCode::Char('f'));

    assert!(!app.files_panel_visible);
    assert_eq!(app.focus, FocusPane::Diff);
    assert_eq!(app.selected_file_index(), 1);
    assert_eq!(app.files_panel.scroll(), 1);
    assert_eq!(app.selected_diff.scroll(), 3);
}

#[test]
fn hidden_files_panel_cannot_receive_keyboard_focus() {
    let mut app = app_with(changeset_with_one_file());
    press_key(&mut app, KeyCode::Char('f'));

    press_key(&mut app, KeyCode::Tab);
    assert_eq!(app.focus, FocusPane::Diff);

    press_key(&mut app, KeyCode::Left);
    assert_eq!(app.focus, FocusPane::Diff);
}

#[test]
fn showing_files_panel_moves_focus_to_sidebar() {
    let mut app = app_with(changeset_with_one_file());
    press_key(&mut app, KeyCode::Char('f'));

    press_key(&mut app, KeyCode::Char('f'));

    assert!(app.files_panel_visible);
    assert_eq!(app.focus, FocusPane::Sidebar);
}

#[test]
fn tab_and_backtab_cycle_through_every_pane() {
    let mut app = app_with_repository_overview();

    for expected in [
        FocusPane::Branches,
        FocusPane::Commits,
        FocusPane::Diff,
        FocusPane::Sidebar,
    ] {
        press_key(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, expected);
    }

    press_key_with_modifiers(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(app.focus, FocusPane::Diff);
}

#[test]
fn repository_panes_have_independent_keyboard_selection_and_copy() {
    let mut app = app_with_repository_overview();
    app.focus = FocusPane::Branches;

    press_key(&mut app, KeyCode::Char('j'));
    assert_repository_copy(&app, RepositoryPane::Branches, "topic");
    press_key(&mut app, KeyCode::Char('y'));
    let branch_copy = take_clipboard_request(&mut app).unwrap();
    assert_eq!(branch_copy.text(), "topic");
    assert_eq!(branch_copy.success_message(), "copied selected branch name");

    app.focus = FocusPane::Commits;
    press_key(&mut app, KeyCode::Char('j'));
    assert_repository_copy(&app, RepositoryPane::Commits, "def5678");
    press_key(&mut app, KeyCode::Char('y'));
    let commit_copy = take_clipboard_request(&mut app).unwrap();
    assert_eq!(commit_copy.text(), "def5678");
    assert_eq!(commit_copy.success_message(), "copied selected commit id");
}

#[test]
fn repository_focus_adds_contextual_copy_hint_to_footer() {
    let mut app = app_with_repository_overview();
    app.focus = FocusPane::Branches;
    let theme = Theme::github_dark();

    assert!(line_text(&app.keybind_bar_line(theme)).contains("copy branch"));

    app.focus = FocusPane::Commits;
    assert!(line_text(&app.keybind_bar_line(theme)).contains("copy commit"));
}

#[test]
fn keyboard_commit_selection_loads_and_renders_commit_details() {
    let mut app = app_with_repository_overview();
    app.focus = FocusPane::Branches;

    press_key(&mut app, KeyCode::Tab);

    assert_eq!(app.focus, FocusPane::Commits);
    assert_eq!(take_commit_load(&mut app).as_deref(), Some("abc1234"));
    let loading = render_tall_diff_pane(&mut app, Theme::github_dark());
    assert!(pane_text(&loading).contains("Loading commit details"));

    app.handle_runtime_event(RuntimeEvent::CommitDetailsFinished {
        revision: "abc1234".to_string(),
        outcome: CommitDetailsOutcome::Loaded(commit_details(
            "abc1234000000000000000000000000000000000",
            "First full message\n\nBody",
            "old line",
        )),
    });
    let pane = render_tall_diff_pane(&mut app, Theme::github_dark());
    let text = pane_text(&pane);
    assert!(text.contains("Commit: abc1234000000000000000000000000000000000"));
    assert!(text.contains("Author: Chunk Test"));
    assert!(text.contains("Date: 2026-08-04T12:00:00-04:00"));
    assert!(text.contains("First full message"));
    assert!(text.contains("Files: 1 file changed, +1 -1"));
    assert!(text.contains("sample.txt"));
    assert!(text.contains("old line"));
}

#[test]
fn moving_commit_selection_ignores_stale_loaded_content() {
    let mut app = app_with_repository_overview();
    app.focus = FocusPane::Commits;

    press_key(&mut app, KeyCode::Char('j'));

    assert_eq!(take_commit_load(&mut app).as_deref(), Some("def5678"));
    app.handle_runtime_event(RuntimeEvent::CommitDetailsFinished {
        revision: "abc1234".to_string(),
        outcome: CommitDetailsOutcome::Loaded(commit_details(
            "abc1234000000000000000000000000000000000",
            "stale message",
            "stale diff",
        )),
    });
    let pane = render_tall_diff_pane(&mut app, Theme::github_dark());
    assert!(pane_text(&pane).contains("Loading commit details"));
    assert!(!pane_text(&pane).contains("stale message"));

    app.handle_runtime_event(RuntimeEvent::CommitDetailsFinished {
        revision: "def5678".to_string(),
        outcome: CommitDetailsOutcome::Loaded(commit_details(
            "def5678000000000000000000000000000000000",
            "current message",
            "current diff",
        )),
    });
    let pane = render_tall_diff_pane(&mut app, Theme::github_dark());
    assert!(pane_text(&pane).contains("current message"));
}

#[test]
fn commit_git_error_is_shown_and_files_focus_restores_worktree_diff() {
    let mut app = app_with_repository_overview();
    app.focus = FocusPane::Branches;
    press_key(&mut app, KeyCode::Tab);
    take_commit_load(&mut app);
    app.handle_runtime_event(RuntimeEvent::CommitDetailsFinished {
        revision: "abc1234".to_string(),
        outcome: CommitDetailsOutcome::Failed("unknown revision".to_string()),
    });

    let failed = render_tall_diff_pane(&mut app, Theme::github_dark());
    assert!(pane_text(&failed).contains("Git error: unknown revision"));

    press_key(&mut app, KeyCode::Left);

    assert_eq!(app.focus, FocusPane::Sidebar);
    assert!(take_commit_cancel(&mut app));
    let worktree = render_tall_diff_pane(&mut app, Theme::github_dark());
    assert!(!pane_text(&worktree).contains("Git error"));
    assert!(worktree.title.contains("changeset"));
}

#[test]
fn only_the_focused_list_pane_highlights_its_selection() {
    let mut app = app_with_repository_overview();
    let theme = Theme::github_dark();

    let files = render_sidebar(&mut app);
    let branches = app.repository_panel_rows(
        RepositoryPane::Branches,
        Rect::new(0, 4, 40, 4),
        38,
        2,
        theme,
    );
    assert!(files.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.style.bg == Some(theme.selected))
    }));
    assert!(branches.iter().all(|line| {
        line.spans
            .iter()
            .all(|span| span.style.bg != Some(theme.selected))
    }));

    app.focus = FocusPane::Branches;
    let files = render_sidebar(&mut app);
    let branches = app.repository_panel_rows(
        RepositoryPane::Branches,
        Rect::new(0, 4, 40, 4),
        38,
        2,
        theme,
    );
    let commits = app.repository_panel_rows(
        RepositoryPane::Commits,
        Rect::new(0, 9, 40, 4),
        38,
        2,
        theme,
    );
    assert!(files.iter().all(|line| {
        line.spans
            .iter()
            .all(|span| span.style.bg != Some(theme.selected))
    }));
    assert!(
        branches[0]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(theme.selected))
    );
    assert!(commits.iter().all(|line| {
        line.spans
            .iter()
            .all(|span| span.style.bg != Some(theme.selected))
    }));
}

#[test]
fn repository_panel_mouse_selects_and_scrolls_the_pointed_list() {
    let mut app = app_with_repository_overview();
    render_repository_panels(&mut app, 2);

    app.handle_left_down(1, 11);
    app.handle_left_up(1, 11);
    assert_eq!(app.focus, FocusPane::Commits);
    assert_repository_copy(&app, RepositoryPane::Commits, "def5678");
    assert_eq!(take_commit_load(&mut app).as_deref(), Some("def5678"));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 1,
        row: 5,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.focus, FocusPane::Branches);
    assert_repository_copy(&app, RepositoryPane::Branches, "topic");
    assert_repository_copy(&app, RepositoryPane::Commits, "def5678");
}

#[test]
fn repository_panel_scroll_keeps_keyboard_selection_visible() {
    let mut app = app_with_repository_overview();
    let mut overview = repository_overview();
    overview.branches.extend([
        LocalBranch {
            name: "release".to_string(),
            current: false,
        },
        LocalBranch {
            name: "next".to_string(),
            current: false,
        },
    ]);
    app.apply_repository_overview(overview);
    app.focus = FocusPane::Branches;
    for _ in 0..3 {
        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
            .unwrap();
    }

    let lines = app.repository_panel_rows(
        RepositoryPane::Branches,
        Rect::new(0, 4, 40, 4),
        38,
        2,
        Theme::github_dark(),
    );

    assert_eq!(line_text(&lines[1]).trim_end(), "  next");
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|span| span.style.bg == Some(Theme::github_dark().selected))
    );
}

#[test]
fn repository_reload_preserves_selected_identities_after_reordering() {
    let mut app = app_with_repository_overview();
    assert!(app.repository_panels.move_selection(
        RepositoryPane::Branches,
        VerticalDirection::Down,
        1,
    ));
    assert!(app.repository_panels.move_selection(
        RepositoryPane::Commits,
        VerticalDirection::Down,
        1,
    ));

    app.apply_repository_overview(RepositoryOverview {
        branches: vec![
            LocalBranch {
                name: "topic".to_string(),
                current: false,
            },
            LocalBranch {
                name: "main".to_string(),
                current: true,
            },
        ],
        commits: vec![
            CommitSummary {
                short_id: "def5678".to_string(),
                subject: "Second".to_string(),
            },
            CommitSummary {
                short_id: "abc1234".to_string(),
                subject: "First".to_string(),
            },
        ],
    });

    assert_repository_copy(&app, RepositoryPane::Branches, "topic");
    assert_repository_copy(&app, RepositoryPane::Commits, "def5678");
}

#[test]
fn initial_files_panel_focuses_root_folder() {
    let app = app_with(changeset_with_paths(["src/app.rs", "README.md"]));

    assert_eq!(app.focus, FocusPane::Sidebar);
    assert_eq!(app.files_panel.current_folder_path(), Some(""));
}

#[test]
fn initial_files_panel_focuses_first_folder_when_root_is_hidden() {
    let app = app_with(changeset_with_paths(["src/app.rs", "src/main.rs"]));

    assert_eq!(app.focus, FocusPane::Sidebar);
    assert_eq!(app.files_panel.current_folder_path(), Some("src"));
}

#[test]
fn tree_navigation_skips_files_hidden_by_collapsed_folders() {
    let mut app = app_with(changeset_with_paths([
        "src/app.rs",
        "src/bin/chunk.rs",
        "src/config.rs",
        "README.md",
    ]));
    app.files_panel.collapse("src/bin");
    app.files_panel
        .set_selected_file(0, app.changeset.files.len());
    app.files_panel.set_cursor_file(0);

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    assert_selected_file_path(&app, "src/config.rs");

    app.files_panel
        .set_selected_file(1, app.changeset.files.len());
    app.files_panel.clear_cursor();
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    assert_selected_file_path(&app, "src/config.rs");
}

#[test]
fn clicking_tree_file_selects_corresponding_diff() {
    let mut app = app_with(changeset_with_paths([
        "src/app.rs",
        "src/bin/chunk.rs",
        "src/config.rs",
        "README.md",
    ]));
    app.files_panel
        .set_selected_file(1, app.changeset.files.len());
    render_sidebar(&mut app);

    app.handle_left_down(1, 6);
    app.handle_left_up(1, 6);

    assert_eq!(app.focus, FocusPane::Sidebar);
    assert_selected_file_path(&app, "src/app.rs");
}

#[test]
fn clicking_tree_folder_collapses_without_losing_selected_file() {
    let mut app = app_with(changeset_with_paths([
        "src/app.rs",
        "src/bin/chunk.rs",
        "src/config.rs",
        "README.md",
    ]));
    app.files_panel
        .set_selected_file(1, app.changeset.files.len());
    render_sidebar(&mut app);

    app.handle_left_down(1, 4);
    app.handle_left_up(1, 4);

    assert!(app.files_panel.is_collapsed("src/bin"));
    assert_selected_file_path(&app, "src/bin/chunk.rs");

    let lines = render_sidebar(&mut app);
    let sidebar = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
    assert!(sidebar.contains("bin/"));
    assert!(!sidebar.contains("chunk.rs"));
}

#[test]
fn question_mark_toggles_help_overlay() {
    let mut app = app_with(changeset_with_one_file());

    press_key(&mut app, KeyCode::Char('?'));
    assert!(app.help_overlay_visible());

    press_key(&mut app, KeyCode::Char('?'));
    assert!(!app.help_overlay_visible());
}

#[test]
fn help_overlay_dismisses_without_exiting() {
    let mut app = app_with(changeset_with_one_file());

    press_key(&mut app, KeyCode::Char('?'));
    let keep_running = press_key(&mut app, KeyCode::Esc);
    assert!(keep_running);
    assert!(!app.help_overlay_visible());

    press_key(&mut app, KeyCode::Char('?'));
    let keep_running = press_key(&mut app, KeyCode::Char('q'));
    assert!(keep_running);
    assert!(!app.help_overlay_visible());
}

#[test]
fn ctrl_c_exits_tui() {
    let mut app = app_with(changeset_with_one_file());

    let keep_running =
        press_key_with_modifiers(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);

    assert!(!keep_running);
}

#[test]
fn hunk_jump_uses_cached_wrapped_offsets() {
    let mut app = app_with(changeset_with_two_hunk_file());
    let theme = Theme::github_dark();
    app.selected_diff.begin_test_frame(Rect::default(), 3);
    app.selected_diff.cache_test_diff_lines(
        0,
        RenderedDiffLines::new(
            "0".to_string(),
            24,
            theme.syntax,
            true,
            vec![Line::raw("row"); 10],
            false,
        )
        .with_hunk_offsets(vec![1, 80]),
    );

    press_key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.selected_diff.scroll(), 79);
    assert_eq!(app.selected_diff.selected_hunk_index(), Some(1));

    press_key(&mut app, KeyCode::Char('N'));
    assert_eq!(app.selected_diff.scroll(), 0);
    assert_eq!(app.selected_diff.selected_hunk_index(), Some(0));
}

#[test]
fn hunk_jump_handles_missing_and_single_offsets() {
    let mut app = app_with(changeset_with_one_file());
    let theme = Theme::github_dark();
    app.selected_diff.begin_test_frame(Rect::default(), 3);
    app.selected_diff.cache_test_diff_lines(
        0,
        RenderedDiffLines::new(
            "0".to_string(),
            24,
            theme.syntax,
            true,
            vec![Line::raw("row"); 8],
            true,
        )
        .with_hunk_offsets(Vec::new()),
    );
    app.selected_diff.set_scroll(4);

    press_key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.selected_diff.scroll(), 4);

    app.selected_diff.cache_test_diff_lines(
        0,
        RenderedDiffLines::new(
            "0".to_string(),
            24,
            theme.syntax,
            true,
            vec![Line::raw("row"); 8],
            true,
        )
        .with_hunk_offsets(vec![5]),
    );
    app.selected_diff.set_scroll(0);

    press_key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.selected_diff.scroll(), 4);

    press_key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.selected_diff.scroll(), 4);
}

#[test]
fn scrolling_diff_selects_hunk_at_top_visible_row() {
    let mut app = app_with(changeset_with_two_hunk_file());
    let theme = Theme::github_dark();
    app.selected_diff.begin_test_frame(Rect::default(), 3);
    app.selected_diff.cache_test_diff_lines(
        0,
        RenderedDiffLines::new(
            "0".to_string(),
            24,
            theme.syntax,
            true,
            vec![Line::raw("row"); 100],
            true,
        )
        .with_hunk_offsets(vec![1, 80]),
    );

    app.scroll_diff_by(VerticalDirection::Down, 80);

    assert_eq!(app.selected_diff.selected_hunk_index(), Some(1));
}

#[test]
fn selected_hunk_style_is_applied_to_visible_cached_rows() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_two_hunk_file());
    app.selected_diff.set_selected_hunk_index(Some(1));
    app.selected_diff.set_scroll(8);

    let pane = render_diff_pane(&mut app, theme);

    assert!(
        pane.lines
            .iter()
            .any(|line| line_text(line).starts_with("> @@ -20 +20 @@"))
    );
}

#[test]
fn diff_click_selects_hunk_under_pointer() {
    let mut app = app_with(changeset_with_two_hunk_file());
    let theme = Theme::github_dark();
    app.selected_diff
        .begin_test_frame(Rect::new(0, 0, 80, 10), 8);
    app.selected_diff.cache_test_diff_lines(
        0,
        RenderedDiffLines::new(
            "0".to_string(),
            80,
            theme.syntax,
            true,
            vec![Line::raw("row"); 12],
            true,
        )
        .with_hunk_offsets(vec![1, 5]),
    );

    app.handle_left_down(1, 6);
    app.handle_left_up(1, 6);

    assert_eq!(app.focus, FocusPane::Diff);
    assert_eq!(app.selected_diff.selected_hunk_index(), Some(1));
    assert_eq!(app.selected_diff.scroll(), 1);
}

#[test]
fn text_drag_requests_clipboard_copy() {
    let mut app = app_with(changeset_with_one_file());
    app.begin_render_frame();
    app.selectable_lines(
        Rect::new(2, 3, 10, 1),
        vec![Line::raw("abcdef")],
        0,
        1,
        Theme::github_dark(),
    );

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
        row: 3,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 5,
        row: 3,
        modifiers: KeyModifiers::NONE,
    });
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 5,
        row: 3,
        modifiers: KeyModifiers::NONE,
    });

    let request =
        take_clipboard_request(&mut app).expect("text drag should request clipboard copy");
    assert_eq!(request.text(), "bcd");
    assert_eq!(request.success_message(), "copied selected text");
}

#[test]
fn sidebar_y_copies_selected_file_path() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    app.files_panel
        .set_selected_file(1, app.changeset.files.len());
    app.focus = FocusPane::Sidebar;

    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
        .unwrap();

    let request = take_clipboard_request(&mut app).expect("selected file path should be copied");
    assert_eq!(request.text(), "b.txt");
    assert_eq!(request.success_message(), "copied selected file path");
}

#[test]
fn diff_copy_keys_copy_hunk_or_file_diff() {
    let mut app = app_with(changeset_with_two_hunk_file());
    app.focus = FocusPane::Diff;

    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
        .unwrap();
    let hunk_request =
        take_clipboard_request(&mut app).expect("selected hunk diff should be copied");
    assert!(
        hunk_request
            .text()
            .contains("diff --git a/sample.txt b/sample.txt")
    );
    assert!(hunk_request.text().contains("@@ -1,8 +1,8 @@"));
    assert!(!hunk_request.text().contains("@@ -20 +20 @@"));
    assert_eq!(hunk_request.success_message(), "copied selected hunk diff");

    app.handle_key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT))
        .unwrap();
    let file_request =
        take_clipboard_request(&mut app).expect("selected file diff should be copied");
    assert!(file_request.text().contains("@@ -1,8 +1,8 @@"));
    assert!(file_request.text().contains("@@ -20 +20 @@"));
    assert_eq!(file_request.success_message(), "copied selected file diff");
}

#[test]
fn diff_pane_editor_request_uses_visible_diff_line() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_editor_target_hunks());
    app.focus = FocusPane::Diff;

    render_diff_pane(&mut app, theme);
    let second_hunk_offset = app
        .selected_diff
        .hunk_offset(0, "0", 1)
        .expect("second hunk offset should be cached");
    app.selected_diff.set_scroll(second_hunk_offset + 2);
    render_diff_pane(&mut app, theme);

    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
        .unwrap();

    let request = take_editor_request(&mut app).expect("editor request should be queued");
    assert!(request.path.ends_with("sample.txt"));
    assert_eq!(request.line, Some(51));
}

#[test]
fn sidebar_editor_request_uses_first_changed_line_fallback() {
    let mut app = app_with(changeset_with_editor_target_hunks());
    app.focus = FocusPane::Sidebar;

    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE))
        .unwrap();

    let request = take_editor_request(&mut app).expect("editor request should be queued");
    assert!(request.path.ends_with("sample.txt"));
    assert_eq!(request.line, Some(2));
}

#[test]
fn diff_scrollbar_click_and_drag_update_scroll() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_file(diff_file("sample.txt", 40)));
    let pane = render_diff_pane(&mut app, theme);
    let scrollbar = pane.scrollbar.expect("large diff should show scrollbar");
    let area = scrollbar.area();

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: area.x,
        row: area.y + area.height - 1,
        modifiers: KeyModifiers::NONE,
    });

    let clicked_scroll = app.selected_diff.scroll();
    assert_eq!(app.focus, FocusPane::Diff);
    assert!(clicked_scroll > 0);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: area.x,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    });

    assert!(app.selected_diff.scroll() < clicked_scroll);
}

#[test]
fn diff_space_without_hunks_sets_live_error_without_exiting() {
    let mut app = app_with(changeset_with_one_file());
    app.focus = FocusPane::Diff;
    app.changeset.files[0].hunks.clear();

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();

    assert!(keep_running);
    assert_eq!(app.live_error.as_deref(), Some("no selected hunk to stage"));
}

#[test]
fn discard_key_requires_confirmation() {
    let mut app = app_with(changeset_with_one_file());
    app.focus = FocusPane::Sidebar;
    app.files_panel.set_cursor_file(0);

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
        .unwrap();

    assert!(matches!(
        app.discard_target(),
        Some(DiscardTarget::File { path, .. }) if path == "sample.txt"
    ));
    assert!(app.live_error.is_none());

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();

    assert!(app.discard_target().is_none());
}

#[test]
fn files_panel_hover_changes_pane_focus_without_changing_tree_focus() {
    let mut app = app_with(changeset_with_paths([
        "src/app.rs",
        "src/bin/chunk.rs",
        "src/config.rs",
        "README.md",
    ]));
    app.focus = FocusPane::Diff;
    app.files_panel
        .set_selected_file(3, app.changeset.files.len());
    app.files_panel.set_cursor_file(3);
    render_sidebar(&mut app);

    app.handle_hover(1, 1);
    assert_eq!(app.focus, FocusPane::Sidebar);
    assert_eq!(app.files_panel.current_file_target(), Some(3));
    assert_selected_file_path(&app, "README.md");

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
        .unwrap();

    assert!(matches!(
        app.discard_target(),
        Some(DiscardTarget::File { path, .. }) if path == "README.md"
    ));
}

#[test]
fn files_panel_hover_changes_focus_on_empty_panel_space() {
    let mut app = app_with(changeset_with_one_file());
    app.focus = FocusPane::Diff;
    render_sidebar(&mut app);

    app.handle_hover(1, 6);

    assert_eq!(app.focus, FocusPane::Sidebar);
    assert_eq!(app.files_panel.current_folder_path(), Some(""));
}

#[test]
fn keyboard_tree_navigation_can_discard_focused_folder() {
    let mut app = app_with(changeset_with_paths([
        "src/app.rs",
        "src/bin/chunk.rs",
        "src/config.rs",
        "README.md",
    ]));
    render_sidebar(&mut app);

    press_chars(&mut app, "jjjd");

    let target = app
        .discard_target()
        .expect("focused folder discard should request confirmation");
    let DiscardTarget::Folder { path, file_paths } = target else {
        panic!("expected folder discard target, got {target:?}");
    };
    assert_eq!(path, "src/bin");
    assert_eq!(file_paths, &vec!["src/bin/chunk.rs".to_string()]);
}

#[test]
fn keyboard_tree_navigation_stages_focused_folder_paths() {
    let mut app = app_with(changeset_with_paths([
        "src/app.rs",
        "src/bin/chunk.rs",
        "src/config.rs",
        "README.md",
    ]));
    render_sidebar(&mut app);

    press_chars(&mut app, "jjj");

    let request = app
        .active_folder_staging_request()
        .expect("focused folder should stage child paths");
    let StagingRequest::Folder {
        path,
        file_paths,
        action,
    } = request
    else {
        panic!("expected folder staging request, got {request:?}");
    };
    assert_eq!(path, "src/bin");
    assert_eq!(file_paths, vec!["src/bin/chunk.rs".to_string()]);
    assert_eq!(action, FolderStagingAction::Stage);
}

#[test]
fn keyboard_tree_navigation_unstages_focused_staged_folder_paths() {
    let mut changeset = changeset_with_paths([
        "src/app.rs",
        "src/bin/chunk.rs",
        "src/config.rs",
        "README.md",
    ]);
    changeset.files[1].stage = crate::model::FileStage::Staged;
    let mut app = app_with(changeset);
    render_sidebar(&mut app);

    press_chars(&mut app, "jjj");

    let request = app
        .active_folder_staging_request()
        .expect("focused staged folder should unstage child paths");
    let StagingRequest::Folder {
        path,
        file_paths,
        action,
    } = request
    else {
        panic!("expected folder staging request, got {request:?}");
    };
    assert_eq!(path, "src/bin");
    assert_eq!(file_paths, vec!["src/bin/chunk.rs".to_string()]);
    assert_eq!(action, FolderStagingAction::Unstage);
}

#[test]
fn search_prompt_applies_query_scrolls_to_first_match_and_highlights_it() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_file(diff_file_with_contents([
        "alpha",
        "target one",
        "beta",
        "target two",
    ])));

    enter_search_query(&mut app, "target");
    let pane = render_diff_pane(&mut app, theme);

    assert_eq!(app.selected_diff.search_match_count(), 2);
    assert_eq!(app.selected_diff.active_search_index(), Some(0));
    let active_row = app.selected_diff.active_search_match_row().unwrap();
    assert!(active_row >= app.selected_diff.scroll());
    assert!(active_row < app.selected_diff.scroll() + app.selected_diff.view_height());
    assert!(pane.lines.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.content.as_ref() == "target" && span.style.bg == Some(theme.accent))
    }));
}

#[test]
fn search_next_and_previous_cycle_matches() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_file(diff_file_with_contents([
        "target one",
        "middle",
        "target two",
    ])));
    enter_search_query(&mut app, "target");
    render_diff_pane(&mut app, theme);

    press_key(&mut app, KeyCode::Char('n'));
    assert_eq!(app.selected_diff.active_search_index(), Some(1));

    press_key(&mut app, KeyCode::Char('N'));
    assert_eq!(app.selected_diff.active_search_index(), Some(0));
}

#[test]
fn esc_clears_active_search_without_exiting() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_file(diff_file_with_contents([
        "target one",
        "middle",
        "target two",
    ])));
    enter_search_query(&mut app, "target");
    render_diff_pane(&mut app, theme);

    let keep_running = press_key(&mut app, KeyCode::Esc);
    press_key(&mut app, KeyCode::Char('n'));

    assert!(keep_running);
    assert!(app.selected_diff.active_search_query().is_none());
    assert_eq!(app.selected_diff.search_match_count(), 0);
    assert_eq!(app.selected_diff.active_search_index(), None);
}

#[test]
fn esc_in_search_prompt_clears_previous_search() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_file(diff_file_with_contents(["target"])));
    enter_search_query(&mut app, "target");
    render_diff_pane(&mut app, theme);

    press_key(&mut app, KeyCode::Char('/'));
    let keep_running = press_key(&mut app, KeyCode::Esc);

    assert!(keep_running);
    assert!(!app.selected_diff.search_prompt_open());
    assert!(app.selected_diff.active_search_query().is_none());
    assert_eq!(app.selected_diff.search_match_count(), 0);
}

#[test]
fn ctrl_c_exits_from_search_prompt() {
    let mut app = app_with(changeset_with_one_file());
    press_key(&mut app, KeyCode::Char('/'));

    let keep_running =
        press_key_with_modifiers(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);

    assert!(!keep_running);
}

#[test]
fn search_no_match_state_is_rendered() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_file(diff_file_with_contents([
        "alpha", "beta",
    ])));

    enter_search_query(&mut app, "missing");
    let pane = render_diff_pane(&mut app, theme);

    assert!(
        pane.lines
            .iter()
            .any(|line| line_text(line).contains("no matches"))
    );
}

#[test]
fn custom_command_key_queues_command_request() {
    let mut app = app_with_config(AppConfig {
        commands: vec![custom_command("C", "commit", "git commit")],
        ..AppConfig::default()
    });

    app.handle_key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT))
        .unwrap();

    let request = take_custom_command_request(&mut app).expect("custom command should be queued");
    assert_eq!(request.label(), "commit");
    assert_eq!(request.command(), "git commit");
    assert!(
        app.command_running()
            .is_some_and(|(command, _, cancelling)| command.label() == "commit" && !cancelling)
    );
}

#[test]
fn remapped_quit_key_quits_and_default_does_not() {
    let mut app = app_with_config(AppConfig {
        keybinds: keybinds_with(&[(BuiltinAction::Quit, "Q")]),
        ..AppConfig::default()
    });

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT))
        .unwrap();
    assert!(!keep_running, "remapped quit key Q should exit");

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(
        keep_running,
        "default quit key q should no longer exit once remapped"
    );
}

#[test]
fn remapped_quit_key_closes_help_overlay_and_default_does_not() {
    let mut app = app_with_config(AppConfig {
        keybinds: keybinds_with(&[(BuiltinAction::Quit, "Q")]),
        ..AppConfig::default()
    });

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.help_overlay_visible());

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT))
        .unwrap();
    assert!(keep_running);
    assert!(!app.help_overlay_visible());

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.help_overlay_visible());
    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(keep_running);
    assert!(
        app.help_overlay_visible(),
        "default q should not close help once quit is remapped"
    );
}

#[test]
fn help_overlay_reflects_remapped_keys() {
    let app = app_with_config(AppConfig {
        keybinds: keybinds_with(&[(BuiltinAction::Quit, "Q"), (BuiltinAction::Help, "H")]),
        ..AppConfig::default()
    });
    let theme = Theme::github_dark();

    let help_lines = app
        .help_overlay_lines(80, theme)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    let help = help_lines.join("\n");

    assert!(
        help_lines
            .iter()
            .any(|line| line.starts_with("H ") && line.contains("help/dismiss")),
        "help was: {help}"
    );
    assert!(
        help_lines
            .iter()
            .any(|line| line.starts_with("Q ") && line.contains("close help or quit")),
        "help was: {help}"
    );
    assert!(!help.contains("? help/dismiss"));
}

#[test]
fn custom_command_runs_on_key_freed_by_remapped_builtin() {
    let mut app = app_with_config(AppConfig {
        keybinds: keybinds_with(&[(BuiltinAction::Quit, "Q")]),
        commands: vec![custom_command("q", "quick", "true")],
        ..AppConfig::default()
    });

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();

    let request = take_custom_command_request(&mut app)
        .expect("freed default key should run the custom command");
    assert_eq!(request.label(), "quick");
}

#[test]
fn footer_reflects_remapped_move_keys() {
    let app = app_with_config(AppConfig {
        keybinds: keybinds_with(&[(BuiltinAction::MoveDown, "J"), (BuiltinAction::MoveUp, "K")]),
        ..AppConfig::default()
    });
    let theme = Theme::github_dark();

    let footer = line_text(&app.keybind_bar_line(theme));
    assert!(footer.contains("J/K"), "footer was: {footer}");
    assert!(!footer.contains("j/k"), "footer was: {footer}");
}

#[test]
fn r_key_toggles_selected_file_review_state() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    assert!(!app.is_selected_file_reviewed());

    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.is_selected_file_reviewed());

    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
        .unwrap();
    assert!(!app.is_selected_file_reviewed());
}

#[test]
fn r_key_reviews_newly_selected_file_independently() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));

    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.is_selected_file_reviewed());

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    assert_selected_file_path(&app, "b.txt");
    assert!(!app.is_selected_file_reviewed());

    app.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.is_selected_file_reviewed());
}

#[test]
fn review_state_survives_reload_by_path_identity() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.is_selected_file_reviewed());

    app.apply_reloaded_changeset(changeset_with_paths(["b.txt", "a.txt"]), true);

    assert_selected_file_path(&app, "a.txt");
    assert!(app.is_selected_file_reviewed());
}

#[test]
fn focused_folder_survives_reload_when_folder_still_exists() {
    let mut app = app_with(changeset_with_paths([
        "src/app.rs",
        "src/bin/chunk.rs",
        "src/config.rs",
        "README.md",
    ]));
    app.files_panel.set_cursor_folder("src/bin");

    app.apply_reloaded_changeset(
        changeset_with_paths(["README.md", "src/bin/chunk.rs", "src/app.rs"]),
        false,
    );

    assert_eq!(app.files_panel.current_folder_path(), Some("src/bin"));
}

#[test]
fn r_key_works_from_diff_pane_focus() {
    let mut app = app_with(changeset_with_one_file());
    app.focus = FocusPane::Diff;
    assert!(!app.is_selected_file_reviewed());

    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.is_selected_file_reviewed());
}

#[test]
fn custom_commands_are_help_only_not_footer_hints() {
    let app = app_with_config(AppConfig {
        commands: vec![custom_command("P", "publish", "git push")],
        ..AppConfig::default()
    });
    let theme = Theme::github_dark();

    let footer = line_text(&app.keybind_bar_line(theme));
    let help = app
        .help_overlay_lines(80, theme)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!footer.contains("publish"), "footer was {footer:?}");
    assert!(help.contains("Custom commands"));
    assert!(help.contains("P  publish  git push"));
}

#[test]
fn footer_keeps_secondary_actions_in_help_only() {
    let app = app_with(changeset_with_one_file());
    let theme = Theme::github_dark();

    let footer = line_text(&app.keybind_bar_line(theme));
    let help = app
        .help_overlay_lines(80, theme)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!footer.contains("discard"), "footer was {footer:?}");
    assert!(!footer.contains("ask AI"), "footer was {footer:?}");
    assert!(!footer.contains("explain"), "footer was {footer:?}");
    assert!(!footer.contains("unstaged"), "footer was {footer:?}");
    assert!(help.contains("discard focused file, folder, or hunk"));
    assert!(help.contains("Ask AI about focused file or hunk"));
    assert!(help.contains("Explain focused file or hunk with Ask AI"));
    assert!(help.contains("Summarize unpublished changes with Ask AI"));
    assert!(help.contains("copy selected hunk diff"));
    assert!(help.contains("copy selected file diff"));
}

#[test]
fn custom_command_running_indicator_is_replaced_by_output() {
    let mut app = app_with(changeset_with_one_file());
    let command = custom_command("C", "commit and push", "git commit && git push");

    app.set_custom_command_running(&command);
    let running_pane = render_diff_pane(&mut app, Theme::github_dark());
    let running_text = pane_text(&running_pane);

    assert!(running_text.contains("⠋ Running command: commit and push"));

    app.advance_custom_command_spinner();
    let next_running_pane = render_diff_pane(&mut app, Theme::github_dark());
    let next_running_text = pane_text(&next_running_pane);

    assert!(next_running_text.contains("⠙ Running command: commit and push"));

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();

    assert!(keep_running);
    assert!(app.command_running().is_some());

    app.set_custom_command_result(CustomCommandResult::not_started(&command, None, "failed"));
    let output_pane = render_diff_pane(&mut app, Theme::github_dark());
    let output_text = pane_text(&output_pane);

    assert!(app.command_running().is_none());
    assert!(output_pane.title.contains("Command: commit and push"));
    assert!(!output_text.contains("Running command: commit and push"));
}

#[test]
fn ctrl_c_cancels_running_custom_command() {
    let mut app = app_with(changeset_with_one_file());
    let command = custom_command("C", "long command", "sleep 60");

    app.set_custom_command_running(&command);

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .unwrap();

    assert!(keep_running);
    assert!(take_custom_command_cancel_request(&mut app));
    assert!(
        app.command_running()
            .is_some_and(|(_, _, cancelling)| cancelling)
    );
}

#[test]
fn command_output_pane_scrolls_and_closes() {
    let mut app = app_with(changeset_with_one_file());
    let command = custom_command("C", "long output", "false");
    app.set_custom_command_result(CustomCommandResult::not_started(
        &command,
        None,
        "one\ntwo\nthree\nfour\nfive\nsix\nseven",
    ));

    let pane = render_diff_pane(&mut app, Theme::github_dark());
    assert!(pane.title.contains("Command: long output"));
    assert_eq!(
        app.command_output().map(|output| output.scroll.offset()),
        Some(0)
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        app.command_output().map(|output| output.scroll.offset()),
        Some(1)
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.command_output().is_none());
}

#[test]
fn ask_ai_key_from_files_panel_queues_file_context() {
    let mut app = app_with_ask_ai_review();

    press_key(&mut app, KeyCode::Char('a'));
    let prompt_pane = render_diff_pane(&mut app, Theme::github_dark());

    assert!(pane_text(&prompt_pane).contains("Ask AI: type a question"));
    assert!(matches!(app.overlay, Some(Overlay::AskAiPrompt(_))));

    enter_ask_ai_question(&mut app, "Why changed?");

    let request = take_ask_ai_request(&mut app).expect("Ask AI request should be queued");
    assert_eq!(request.question(), "Why changed?");
    assert_eq!(request.context().summary(), "sample.txt");
    assert_ask_ai_running(&app, request.question());
}

#[test]
fn ask_ai_request_uses_configured_harness() {
    let mut app = app_with_config(AppConfig {
        ai: AiSettings::with_harness(AiHarness::Codex),
        ..AppConfig::default()
    });

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .unwrap();
    enter_ask_ai_question(&mut app, "Why changed?");

    let (harness, request) = take_ask_ai_effect(&mut app).expect("Ask AI request should be queued");
    assert_eq!(harness, AiHarness::Codex);
    assert_eq!(request.question(), "Why changed?");
}

#[test]
fn ask_ai_key_from_diff_pane_queues_hunk_context() {
    let mut app = app_with_ask_ai_review();
    app.focus = FocusPane::Diff;

    press_key(&mut app, KeyCode::Char('a'));
    let prompt_pane = render_diff_pane(&mut app, Theme::github_dark());

    assert!(pane_text(&prompt_pane).contains("Ask AI: type a question"));
    assert!(matches!(app.overlay, Some(Overlay::AskAiPrompt(_))));

    enter_ask_ai_question(&mut app, "Why changed?");

    let request = take_ask_ai_request(&mut app).expect("Ask AI request should be queued");
    assert_eq!(request.question(), "Why changed?");
    assert_eq!(request.context().summary(), "sample.txt hunk 1");
    assert_ask_ai_running(&app, request.question());
}

#[test]
fn explain_code_key_from_files_panel_queues_file_context() {
    let mut app = app_with_ask_ai_review();

    press_key(&mut app, KeyCode::Char('x'));

    let request = take_ask_ai_request(&mut app).expect("Explain Code request should be queued");
    assert_explain_code_question(request.question());
    assert_eq!(request.context().summary(), "sample.txt");
    assert_ask_ai_running(&app, request.question());
}

#[test]
fn explain_code_key_from_diff_pane_queues_hunk_context() {
    let mut app = app_with_ask_ai_review();
    app.focus = FocusPane::Diff;

    press_key(&mut app, KeyCode::Char('x'));

    let request = take_ask_ai_request(&mut app).expect("Explain Code request should be queued");
    assert_explain_code_question(request.question());
    assert_eq!(request.context().summary(), "sample.txt hunk 1");
    assert_ask_ai_running(&app, request.question());
}

#[test]
fn unpublished_summary_key_queues_tui_ai_request() {
    let mut app = app_with(changeset_with_one_file());
    app.focus = FocusPane::Sidebar;

    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE))
        .unwrap();

    assert!(take_unpublished_summary_request(&mut app));
    assert_eq!(app.focus, FocusPane::Diff);
    assert!(app.live_error.is_none());
    assert!(
        app.ask_ai_running()
            .is_some_and(|(question, _, cancelling)| question
                == crate::ask_ai::UNPUBLISHED_SUMMARY_QUESTION
                && !cancelling)
    );
}

#[test]
fn unpublished_summary_uses_configured_harness() {
    let mut app = app_with_config(AppConfig {
        ai: AiSettings::with_harness(AiHarness::ClaudeCode),
        ..AppConfig::default()
    });

    app.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE))
        .unwrap();

    assert_eq!(
        take_unpublished_summary_effect(&mut app),
        Some(AiHarness::ClaudeCode)
    );
}

#[test]
fn f2_harness_picker_sets_harness_and_queues_config_save() {
    let mut app = app_with(changeset_with_one_file());

    drive_harness_picker(&mut app, &[KeyCode::Char('3'), KeyCode::Enter]);

    assert_eq!(app.ai_settings.harness, AiHarness::Codex);
    assert!(!app.harness_picker_visible());
    assert_eq!(
        take_save_ai_harness_effect(&mut app),
        Some(AiHarness::Codex)
    );
}

#[test]
fn harness_picker_enter_applies_highlighted_harness() {
    let mut app = app_with(changeset_with_one_file());

    drive_harness_picker(&mut app, &[KeyCode::Right, KeyCode::Enter]);

    assert_eq!(app.ai_settings.harness, AiHarness::ClaudeCode);
    assert_eq!(
        take_save_ai_harness_effect(&mut app),
        Some(AiHarness::ClaudeCode)
    );
}

#[test]
fn harness_picker_configures_model_and_native_effort() {
    let mut app = app_with(changeset_with_one_file());

    drive_harness_picker(
        &mut app,
        &[
            KeyCode::Char('3'),
            KeyCode::Down,
            KeyCode::Right,
            KeyCode::Down,
            KeyCode::Right,
            KeyCode::Enter,
        ],
    );

    assert_eq!(app.ai_settings.harness, AiHarness::Codex);
    assert_eq!(app.ai_settings.codex.model(), Some("gpt-5.6-sol"));
    assert_eq!(app.ai_settings.codex.effort(), Some("low"));
    assert!(matches!(
        app.effects.pop_front(),
        Some(AppEffect::SaveAiSettings(settings))
            if settings.codex.model() == Some("gpt-5.6-sol")
                && settings.codex.effort() == Some("low")
    ));
}

#[test]
fn unpublished_summary_running_uses_ask_ai_pane() {
    let mut app = app_with(changeset_with_one_file());

    app.set_ask_ai_running_question(crate::ask_ai::UNPUBLISHED_SUMMARY_QUESTION);
    let running_pane = render_diff_pane(&mut app, Theme::github_dark());

    assert!(pane_text(&running_pane).contains("Asking AI: Summarize unpublished changes"));
}

#[test]
fn ask_ai_running_can_be_cancelled() {
    let mut app = app_with(changeset_with_one_file());
    let request = ask_ai_request("Explain this");

    app.set_ask_ai_running(&request);
    let running_pane = render_diff_pane(&mut app, Theme::github_dark());
    assert!(pane_text(&running_pane).contains("Asking AI: Explain this"));

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();

    assert!(keep_running);
    assert!(matches!(
        app.overlay,
        Some(Overlay::AskAiRunning {
            cancelling: true,
            ..
        })
    ));
    assert!(take_ask_ai_cancel_request(&mut app));
    assert!(!take_ask_ai_cancel_request(&mut app));
}

#[test]
fn ask_ai_output_pane_scrolls_and_closes() {
    let mut app = app_with(changeset_with_one_file());
    let request = ask_ai_request("Explain this");
    app.set_ask_ai_result(AskAiResult::not_started(
        request,
        None,
        "one\ntwo\nthree\nfour\nfive\nsix\nseven",
    ));

    let pane = render_diff_pane(&mut app, Theme::github_dark());
    assert!(pane.title.contains("Ask AI: sample.txt hunk 1"));
    assert!(pane_text(&pane).contains("question: Explain this"));
    assert_eq!(
        app.ask_ai_output().map(|output| output.scroll.offset()),
        Some(0)
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        app.ask_ai_output().map(|output| output.scroll.offset()),
        Some(1)
    );

    app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.ask_ai_output().is_none());
}

#[test]
fn ask_ai_output_y_copies_answer_text() {
    let mut app = app_with(changeset_with_one_file());
    let request = ask_ai_request("Explain this");
    app.set_ask_ai_result(AskAiResult::from_output(
        request,
        std::path::PathBuf::from("."),
        output_status(0, "answer\n", ""),
    ));

    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
        .unwrap();

    let request = take_clipboard_request(&mut app).expect("Ask AI answer should be copied");
    assert_eq!(request.text(), "answer\n");
    assert_eq!(request.success_message(), "copied Ask AI answer");
    assert!(app.ask_ai_output().is_some());

    app.set_live_notice(request.success_message().to_string());
    let pane = render_diff_pane(&mut app, Theme::github_dark());
    assert!(pane_text(&pane).contains("ok copied Ask AI answer"));
}

fn app_with(changeset: Changeset) -> App {
    App::new(LoadedReview::worktree(changeset))
}

fn app_with_ask_ai_review() -> App {
    let mut changeset = changeset_with_one_file();
    changeset.title = "Tracked changes".to_string();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    app_with(changeset)
}

fn app_with_repository_overview() -> App {
    App::new(LoadedReview::worktree_with_repository_overview(
        changeset_with_one_file(),
        repository_overview(),
    ))
}

fn repository_overview() -> RepositoryOverview {
    RepositoryOverview {
        branches: vec![
            LocalBranch {
                name: "main".to_string(),
                current: true,
            },
            LocalBranch {
                name: "topic".to_string(),
                current: false,
            },
        ],
        commits: vec![
            CommitSummary {
                short_id: "abc1234".to_string(),
                subject: "First".to_string(),
            },
            CommitSummary {
                short_id: "def5678".to_string(),
                subject: "Second".to_string(),
            },
        ],
    }
}

fn drive_harness_picker(app: &mut App, keys: &[KeyCode]) {
    press_key(app, KeyCode::F(2));
    assert!(app.harness_picker_visible());

    for &key in keys {
        press_key(app, key);
    }
}

fn press_key(app: &mut App, code: KeyCode) -> bool {
    press_key_with_modifiers(app, code, KeyModifiers::NONE)
}

fn press_key_with_modifiers(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    app.handle_key(KeyEvent::new(code, modifiers)).unwrap()
}

fn press_chars(app: &mut App, characters: &str) {
    for character in characters.chars() {
        press_key(app, KeyCode::Char(character));
    }
}

fn take_effect<T>(app: &mut App, extract: impl FnMut(AppEffect) -> Option<T>) -> Option<T> {
    app.take_effects().into_iter().find_map(extract)
}

fn take_clipboard_request(app: &mut App) -> Option<ClipboardRequest> {
    take_effect(app, |effect| match effect {
        AppEffect::CopyToClipboard(request) => Some(request),
        _ => None,
    })
}

fn take_commit_load(app: &mut App) -> Option<String> {
    take_effect(app, |effect| match effect {
        AppEffect::LoadCommitDetails { revision } => Some(revision),
        _ => None,
    })
}

fn take_commit_cancel(app: &mut App) -> bool {
    take_effect(app, |effect| {
        matches!(effect, AppEffect::CancelCommitDetailsLoad).then_some(())
    })
    .is_some()
}

fn take_editor_request(app: &mut App) -> Option<crate::editor::EditorRequest> {
    take_effect(app, |effect| match effect {
        AppEffect::OpenEditor(request) => Some(request),
        _ => None,
    })
}

fn take_custom_command_request(app: &mut App) -> Option<CustomCommandBinding> {
    take_effect(app, |effect| match effect {
        AppEffect::RunCustomCommand(command) => Some(command),
        _ => None,
    })
}

fn take_custom_command_cancel_request(app: &mut App) -> bool {
    take_effect(app, |effect| {
        matches!(effect, AppEffect::CancelCustomCommand).then_some(())
    })
    .is_some()
}

fn take_ask_ai_request(app: &mut App) -> Option<AskAiRequest> {
    take_effect(app, |effect| match effect {
        AppEffect::RunAskAi { request, .. } => Some(request),
        _ => None,
    })
}

fn take_ask_ai_effect(app: &mut App) -> Option<(AiHarness, AskAiRequest)> {
    take_effect(app, |effect| match effect {
        AppEffect::RunAskAi { settings, request } => Some((settings.harness, request)),
        _ => None,
    })
}

fn take_ask_ai_cancel_request(app: &mut App) -> bool {
    take_effect(app, |effect| {
        matches!(effect, AppEffect::CancelAskAi).then_some(())
    })
    .is_some()
}

fn take_unpublished_summary_request(app: &mut App) -> bool {
    take_effect(app, |effect| {
        matches!(effect, AppEffect::RunUnpublishedSummary { .. }).then_some(())
    })
    .is_some()
}

fn take_unpublished_summary_effect(app: &mut App) -> Option<AiHarness> {
    take_effect(app, |effect| match effect {
        AppEffect::RunUnpublishedSummary { settings } => Some(settings.harness),
        _ => None,
    })
}

fn take_save_ai_harness_effect(app: &mut App) -> Option<AiHarness> {
    take_effect(app, |effect| match effect {
        AppEffect::SaveAiSettings(settings) => Some(settings.harness),
        _ => None,
    })
}

fn app_with_config(config: AppConfig) -> App {
    App::with_config(LoadedReview::worktree(changeset_with_one_file()), config)
}

fn custom_command(key: &str, label: &str, command: &str) -> CustomCommandBinding {
    CustomCommandBinding::new(
        crate::custom_command::CommandKey::parse(key).unwrap(),
        label.to_string(),
        command.to_string(),
    )
}

fn keybinds_with(overrides: &[(BuiltinAction, &str)]) -> KeybindMap {
    let mut keybinds = KeybindMap::defaults();
    for &(action, key) in overrides {
        keybinds.set(action, BuiltinKey::parse(key).unwrap());
    }
    keybinds
}

fn enter_search_query(app: &mut App, query: &str) {
    press_key(app, KeyCode::Char('/'));
    press_chars(app, query);
    press_key(app, KeyCode::Enter);
}

fn enter_ask_ai_question(app: &mut App, question: &str) {
    press_chars(app, question);
    press_key(app, KeyCode::Enter);
}

fn ask_ai_request(question: &str) -> AskAiRequest {
    let file = diff_file("sample.txt", 1);
    let context = AskAiContext::focused(
        AskAiReviewMode::Worktree,
        "Tracked changes".to_string(),
        "git diff HEAD + untracked".to_string(),
        &file,
        Some(0),
        None,
    );

    AskAiRequest::new(question.to_string(), context)
}

fn output_status(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;

    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

fn assert_explain_code_question(question: &str) {
    assert!(question.contains("Explain the selected or focused code"));
    assert!(question.contains("what the code does"));
    assert!(question.contains("why the changed code matters"));
    assert!(question.contains("assumptions or risks"));
    assert!(question.contains("read-only"));
}

fn assert_ask_ai_running(app: &App, question: &str) {
    assert!(
        app.ask_ai_running()
            .is_some_and(|(running_question, _, cancelling)| {
                running_question == question && !cancelling
            })
    );
}

fn assert_selected_file_path(app: &App, expected_path: &str) {
    assert_eq!(
        app.selected_file().map(DiffFile::display_path),
        Some(expected_path)
    );
}

fn render_diff_pane(app: &mut App, theme: Theme) -> DiffPaneRows {
    app.diff_pane_rows(Rect::new(0, 0, 80, 8), 80, 6, theme)
}

fn render_tall_diff_pane(app: &mut App, theme: Theme) -> DiffPaneRows {
    app.diff_pane_rows(Rect::new(0, 0, 100, 50), 98, 48, theme)
}

fn render_sidebar(app: &mut App) -> Vec<Line<'static>> {
    app.sidebar_rows(Rect::new(0, 0, 40, 8), 38, 6, Theme::github_dark())
}

fn render_repository_panels(app: &mut App, visible_height: usize) {
    app.repository_panel_rows(
        RepositoryPane::Branches,
        Rect::new(0, 4, 40, 4),
        38,
        visible_height,
        Theme::github_dark(),
    );
    app.repository_panel_rows(
        RepositoryPane::Commits,
        Rect::new(0, 9, 40, 4),
        38,
        visible_height,
        Theme::github_dark(),
    );
}

fn assert_repository_copy(app: &App, pane: RepositoryPane, expected: &str) {
    let copy = app
        .repository_panels
        .copy(pane)
        .expect("repository selection should be copyable");
    assert_eq!(copy.into_parts().0, expected);
}

fn pane_text(pane: &DiffPaneRows) -> String {
    pane.lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn changeset_with_file(file: DiffFile) -> Changeset {
    Changeset {
        title: String::new(),
        source_label: String::new(),
        files: vec![file],
    }
}

fn commit_details(id: &str, message: &str, removed_line: &str) -> CommitDetails {
    let mut file = diff_file("sample.txt", 1);
    file.additions = 1;
    file.deletions = 1;
    file.hunks[0].lines = vec![
        DiffLine {
            kind: DiffLineKind::Removed,
            old_line: Some(1),
            new_line: None,
            content: removed_line.to_string(),
        },
        DiffLine {
            kind: DiffLineKind::Added,
            old_line: None,
            new_line: Some(1),
            content: "new line".to_string(),
        },
    ];
    CommitDetails {
        id: id.to_string(),
        author: "Chunk Test".to_string(),
        committed_at: "2026-08-04T12:00:00-04:00".to_string(),
        message: message.to_string(),
        changeset: changeset_with_file(file),
    }
}

fn changeset_with_one_file() -> Changeset {
    changeset_with_paths(["sample.txt"])
}

fn changeset_with_two_hunk_file() -> Changeset {
    let mut changeset = changeset_with_one_file();
    changeset.files[0].hunks.push(DiffHunk {
        header: "@@ -20 +20 @@".to_string(),
        old_start: 20,
        old_lines: 1,
        new_start: 20,
        new_lines: 1,
        stage: crate::model::FileStage::Unstaged,
        lines: vec![DiffLine {
            kind: DiffLineKind::Context,
            old_line: Some(20),
            new_line: Some(20),
            content: "line".to_string(),
        }],
    });
    changeset
}

fn changeset_with_editor_target_hunks() -> Changeset {
    let mut file = diff_file("sample.txt", 1);
    file.additions = 2;
    file.hunks = vec![
        DiffHunk {
            header: "@@ -1,1 +1,2 @@".to_string(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 2,
            stage: crate::model::FileStage::Unstaged,
            lines: vec![context_line(1), added_line(2)],
        },
        DiffHunk {
            header: "@@ -50,6 +50,7 @@".to_string(),
            old_start: 50,
            old_lines: 6,
            new_start: 50,
            new_lines: 7,
            stage: crate::model::FileStage::Unstaged,
            lines: std::iter::once(context_line(50))
                .chain(std::iter::once(added_line(51)))
                .chain((52..=56).map(context_line))
                .collect(),
        },
    ];
    changeset_with_file(file)
}

fn changeset_with_short_file(path: &str) -> Changeset {
    changeset_with_file(diff_file(path, 1))
}

fn changeset_with_paths<const N: usize>(paths: [&str; N]) -> Changeset {
    Changeset {
        title: String::new(),
        source_label: String::new(),
        files: paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                let mut file = diff_file(path, 8);
                file.id = index.to_string();
                file
            })
            .collect(),
    }
}

fn diff_file(path: &str, line_count: u32) -> DiffFile {
    DiffFile {
        id: "0".to_string(),
        old_path: path.to_string(),
        path: path.to_string(),
        old_source: SourceSnapshot::Unloaded,
        new_source: SourceSnapshot::Unloaded,
        status: FileStatus::Modified,
        stage: crate::model::FileStage::Unstaged,
        additions: 0,
        deletions: 0,
        hunks: vec![DiffHunk {
            header: format!("@@ -1,{line_count} +1,{line_count} @@"),
            old_start: 1,
            old_lines: line_count,
            new_start: 1,
            new_lines: line_count,
            stage: crate::model::FileStage::Unstaged,
            lines: (1..=line_count)
                .map(|line_number| DiffLine {
                    kind: DiffLineKind::Context,
                    old_line: Some(line_number),
                    new_line: Some(line_number),
                    content: "line".to_string(),
                })
                .collect(),
        }],
        binary: false,
    }
}

fn diff_file_with_contents<const N: usize>(contents: [&str; N]) -> DiffFile {
    let line_count = contents.len() as u32;
    DiffFile {
        id: "0".to_string(),
        old_path: "sample.txt".to_string(),
        path: "sample.txt".to_string(),
        old_source: SourceSnapshot::Unloaded,
        new_source: SourceSnapshot::Unloaded,
        status: FileStatus::Modified,
        stage: crate::model::FileStage::Unstaged,
        additions: 0,
        deletions: 0,
        hunks: vec![DiffHunk {
            header: format!("@@ -1,{line_count} +1,{line_count} @@"),
            old_start: 1,
            old_lines: line_count,
            new_start: 1,
            new_lines: line_count,
            stage: crate::model::FileStage::Unstaged,
            lines: contents
                .into_iter()
                .enumerate()
                .map(|(index, content)| {
                    let line_number = index as u32 + 1;
                    DiffLine {
                        kind: DiffLineKind::Context,
                        old_line: Some(line_number),
                        new_line: Some(line_number),
                        content: content.to_string(),
                    }
                })
                .collect(),
        }],
        binary: false,
    }
}

fn context_line(line_number: u32) -> DiffLine {
    DiffLine {
        kind: DiffLineKind::Context,
        old_line: Some(line_number),
        new_line: Some(line_number),
        content: "context".to_string(),
    }
}

fn added_line(line_number: u32) -> DiffLine {
    DiffLine {
        kind: DiffLineKind::Added,
        old_line: None,
        new_line: Some(line_number),
        content: "added".to_string(),
    }
}
