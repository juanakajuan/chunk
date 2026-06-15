use super::*;
use crate::ask_ai::{AskAiResult, AskAiReviewMode};
use crate::custom_command::CustomCommandResult;
use crate::model::{DiffHunk, DiffLine, DiffLineKind, FileStatus, SourceSnapshot};
use crate::theme::Theme;
use crate::viewport::RenderedDiffLines;
use ratatui::layout::Rect;
use ratatui::text::Line;

#[test]
fn diff_scroll_bounds_use_rendered_rows_when_available() {
    let mut app = app_with(changeset_with_one_file());
    app.viewport.begin_diff(Rect::default(), 3);
    app.diff_scroll = 99;
    app.viewport.cache_diff_lines(
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

    assert_eq!(app.diff_scroll, 5);
}

#[test]
fn reload_preserves_selected_file_and_scroll_by_path() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    app.selected_file_index = 1;
    app.viewport.begin_diff(Rect::default(), 3);
    app.diff_scroll = 4;

    app.apply_reloaded_changeset(changeset_with_paths(["b.txt", "a.txt"]), true);

    assert_eq!(
        app.selected_file().map(DiffFile::display_path),
        Some("b.txt")
    );
    assert_eq!(app.selected_file_index, 0);
    assert_eq!(app.diff_scroll, 4);
}

#[test]
fn reload_preserves_selected_hunk_by_coordinates() {
    let mut app = app_with(changeset_with_two_hunk_file());
    app.selected_hunk_index = Some(1);

    app.apply_reloaded_changeset(changeset_with_two_hunk_file(), true);

    assert_eq!(app.selected_hunk_index, Some(1));
}

#[test]
fn reload_clamps_scroll_when_selected_file_shrinks() {
    let mut app = app_with(changeset_with_paths(["sample.txt"]));
    app.viewport.begin_diff(Rect::default(), 3);
    app.diff_scroll = 99;

    app.apply_reloaded_changeset(changeset_with_short_file("sample.txt"), true);

    assert_eq!(
        app.selected_file().map(DiffFile::display_path),
        Some("sample.txt")
    );
    assert_eq!(app.diff_scroll, 0);
}

#[test]
fn reload_resets_selection_and_scroll_when_selected_file_disappears() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    app.selected_file_index = 1;
    app.diff_scroll = 4;

    app.apply_reloaded_changeset(changeset_with_paths(["a.txt"]), true);

    assert_eq!(
        app.selected_file().map(DiffFile::display_path),
        Some("a.txt")
    );
    assert_eq!(app.diff_scroll, 0);
}

#[test]
fn hiding_files_panel_moves_focus_to_diff() {
    let mut app = app_with(changeset_with_paths(["a.txt", "b.txt"]));
    app.selected_file_index = 1;
    app.sidebar_scroll = 1;
    app.diff_scroll = 3;

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .unwrap();

    assert!(!app.files_panel_visible);
    assert_eq!(app.focus, FocusPane::Diff);
    assert_eq!(app.selected_file_index, 1);
    assert_eq!(app.sidebar_scroll, 1);
    assert_eq!(app.diff_scroll, 3);
}

#[test]
fn hidden_files_panel_cannot_receive_keyboard_focus() {
    let mut app = app_with(changeset_with_one_file());
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.focus, FocusPane::Diff);

    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.focus, FocusPane::Diff);
}

#[test]
fn showing_files_panel_moves_focus_to_sidebar() {
    let mut app = app_with(changeset_with_one_file());
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .unwrap();

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE))
        .unwrap();

    assert!(app.files_panel_visible);
    assert_eq!(app.focus, FocusPane::Sidebar);
}

#[test]
fn question_mark_toggles_help_overlay() {
    let mut app = app_with(changeset_with_one_file());

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();
    assert!(app.help_overlay_visible());

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();
    assert!(!app.help_overlay_visible());
}

#[test]
fn help_overlay_dismisses_without_exiting() {
    let mut app = app_with(changeset_with_one_file());

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();
    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    assert!(keep_running);
    assert!(!app.help_overlay_visible());

    app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE))
        .unwrap();
    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
        .unwrap();
    assert!(keep_running);
    assert!(!app.help_overlay_visible());
}

#[test]
fn ctrl_c_exits_tui() {
    let mut app = app_with(changeset_with_one_file());

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .unwrap();

    assert!(!keep_running);
}

#[test]
fn hunk_jump_uses_cached_wrapped_offsets() {
    let mut app = app_with(changeset_with_two_hunk_file());
    let theme = Theme::github_dark();
    app.viewport.begin_diff(Rect::default(), 3);
    app.viewport.cache_diff_lines(
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

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.diff_scroll, 79);
    assert_eq!(app.selected_hunk_index, Some(1));

    app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.diff_scroll, 0);
    assert_eq!(app.selected_hunk_index, Some(0));
}

#[test]
fn hunk_jump_handles_missing_and_single_offsets() {
    let mut app = app_with(changeset_with_one_file());
    let theme = Theme::github_dark();
    app.viewport.begin_diff(Rect::default(), 3);
    app.viewport.cache_diff_lines(
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
    app.diff_scroll = 4;

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.diff_scroll, 4);

    app.viewport.cache_diff_lines(
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
    app.diff_scroll = 0;

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.diff_scroll, 4);

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.diff_scroll, 4);
}

#[test]
fn scrolling_diff_selects_hunk_at_top_visible_row() {
    let mut app = app_with(changeset_with_two_hunk_file());
    let theme = Theme::github_dark();
    app.viewport.begin_diff(Rect::default(), 3);
    app.viewport.cache_diff_lines(
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

    assert_eq!(app.selected_hunk_index, Some(1));
}

#[test]
fn selected_hunk_style_is_applied_to_visible_cached_rows() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_two_hunk_file());
    app.selected_hunk_index = Some(1);
    app.diff_scroll = 8;

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
    app.viewport.begin_diff(Rect::new(0, 0, 80, 10), 8);
    app.viewport.cache_diff_lines(
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
    assert_eq!(app.selected_hunk_index, Some(1));
    assert_eq!(app.diff_scroll, 1);
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

    assert_eq!(app.take_clipboard_request().as_deref(), Some("bcd"));
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

    let clicked_scroll = app.diff_scroll;
    assert_eq!(app.focus, FocusPane::Diff);
    assert!(clicked_scroll > 0);

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: area.x,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    });

    assert!(app.diff_scroll < clicked_scroll);
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

    assert_eq!(app.search.match_count(), 2);
    assert_eq!(app.search.active_index(), Some(0));
    let active_row = app.search.active_match_row().unwrap();
    assert!(active_row >= app.diff_scroll);
    assert!(active_row < app.diff_scroll + app.viewport.diff_view_height());
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

    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.search.active_index(), Some(1));

    app.handle_key(KeyEvent::new(KeyCode::Char('N'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(app.search.active_index(), Some(0));
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

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();
    app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
        .unwrap();

    assert!(keep_running);
    assert!(app.search.active_query().is_none());
    assert_eq!(app.search.match_count(), 0);
    assert_eq!(app.search.active_index(), None);
}

#[test]
fn esc_in_search_prompt_clears_previous_search() {
    let theme = Theme::github_dark();
    let mut app = app_with(changeset_with_file(diff_file_with_contents(["target"])));
    enter_search_query(&mut app, "target");
    render_diff_pane(&mut app, theme);

    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .unwrap();
    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .unwrap();

    assert!(keep_running);
    assert!(!app.search.is_prompt_open());
    assert!(app.search.active_query().is_none());
    assert_eq!(app.search.match_count(), 0);
}

#[test]
fn ctrl_c_exits_from_search_prompt() {
    let mut app = app_with(changeset_with_one_file());
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .unwrap();

    let keep_running = app
        .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .unwrap();

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
    });

    app.handle_key(KeyEvent::new(KeyCode::Char('C'), KeyModifiers::SHIFT))
        .unwrap();

    let request = app
        .take_custom_command_request()
        .expect("custom command should be queued");
    assert_eq!(request.label(), "commit");
    assert_eq!(request.command(), "git commit");
}

#[test]
fn custom_commands_are_help_only_not_footer_hints() {
    let app = app_with_config(AppConfig {
        commands: vec![custom_command("P", "publish", "git push")],
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
    assert!(help.contains("P publish  git push"));
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
    assert!(help.contains("d discard focused file or hunk"));
    assert!(help.contains("a Ask AI about focused file or hunk"));
    assert!(help.contains("x Explain focused file or hunk with Ask AI"));
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
    let mut changeset = changeset_with_one_file();
    changeset.title = "Tracked changes".to_string();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    let mut app = app_with(changeset);

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .unwrap();
    let prompt_pane = render_diff_pane(&mut app, Theme::github_dark());

    assert!(pane_text(&prompt_pane).contains("Ask AI: type a question"));
    assert!(matches!(app.overlay, Some(Overlay::AskAiPrompt(_))));

    enter_ask_ai_question(&mut app, "Why changed?");

    let request = app
        .take_ask_ai_request()
        .expect("Ask AI request should be queued");
    assert_eq!(request.question(), "Why changed?");
    assert_eq!(request.context().summary(), "sample.txt");
    assert!(app.overlay.is_none());
}

#[test]
fn ask_ai_key_from_diff_pane_queues_hunk_context() {
    let mut changeset = changeset_with_one_file();
    changeset.title = "Tracked changes".to_string();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    let mut app = app_with(changeset);
    app.focus = FocusPane::Diff;

    app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
        .unwrap();
    let prompt_pane = render_diff_pane(&mut app, Theme::github_dark());

    assert!(pane_text(&prompt_pane).contains("Ask AI: type a question"));
    assert!(matches!(app.overlay, Some(Overlay::AskAiPrompt(_))));

    enter_ask_ai_question(&mut app, "Why changed?");

    let request = app
        .take_ask_ai_request()
        .expect("Ask AI request should be queued");
    assert_eq!(request.question(), "Why changed?");
    assert_eq!(request.context().summary(), "sample.txt hunk 1");
    assert!(app.overlay.is_none());
}

#[test]
fn explain_code_key_from_files_panel_queues_file_context() {
    let mut changeset = changeset_with_one_file();
    changeset.title = "Tracked changes".to_string();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    let mut app = app_with(changeset);

    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
        .unwrap();

    let request = app
        .take_ask_ai_request()
        .expect("Explain Code request should be queued");
    assert_explain_code_question(request.question());
    assert_eq!(request.context().summary(), "sample.txt");
    assert!(app.overlay.is_none());
}

#[test]
fn explain_code_key_from_diff_pane_queues_hunk_context() {
    let mut changeset = changeset_with_one_file();
    changeset.title = "Tracked changes".to_string();
    changeset.source_label = "git diff HEAD + untracked".to_string();
    let mut app = app_with(changeset);
    app.focus = FocusPane::Diff;

    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
        .unwrap();

    let request = app
        .take_ask_ai_request()
        .expect("Explain Code request should be queued");
    assert_explain_code_question(request.question());
    assert_eq!(request.context().summary(), "sample.txt hunk 1");
    assert!(app.overlay.is_none());
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
    assert!(app.take_ask_ai_cancel_request());
    assert!(!app.take_ask_ai_cancel_request());
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

fn app_with(changeset: Changeset) -> App {
    App::new(LoadedReview::worktree(changeset))
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

fn enter_search_query(app: &mut App, query: &str) {
    app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
        .unwrap();
    for character in query.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .unwrap();
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
}

fn enter_ask_ai_question(app: &mut App, question: &str) {
    for character in question.chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
            .unwrap();
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
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

fn assert_explain_code_question(question: &str) {
    assert!(question.contains("Explain the selected or focused code"));
    assert!(question.contains("what the code does"));
    assert!(question.contains("why the changed code matters"));
    assert!(question.contains("assumptions or risks"));
    assert!(question.contains("read-only"));
}

fn render_diff_pane(app: &mut App, theme: Theme) -> DiffPaneRows {
    app.diff_pane_rows(Rect::new(0, 0, 80, 8), 80, 6, theme)
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
