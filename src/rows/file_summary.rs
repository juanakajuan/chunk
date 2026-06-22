use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::model::{DiffFile, FileStage, FileStatus};
use crate::theme::Theme;

use super::text::{color_style, display_width};

pub(super) struct StageDisplay {
    pub(super) suffix: &'static str,
    pub(super) style: Style,
}

pub(super) fn push_stat_spans(
    spans: &mut Vec<Span<'static>>,
    file: &DiffFile,
    background: Color,
    theme: Theme,
) {
    let has_additions = file.additions > 0;
    let has_deletions = file.deletions > 0;

    if has_additions {
        spans.push(Span::styled(
            format!("+{}", file.additions),
            color_style(theme.added, background),
        ));
    }

    if has_additions && has_deletions {
        spans.push(Span::styled(" ", Style::default().bg(background)));
    }

    if has_deletions {
        spans.push(Span::styled(
            format!("-{}", file.deletions),
            color_style(theme.removed, background),
        ));
    }
}

pub(super) fn file_header_label(file: &DiffFile) -> String {
    if has_path_change_label(file) {
        format!("{} -> {}", file.old_path, file.path)
    } else {
        file.display_path().to_string()
    }
}

pub(super) fn sidebar_file_label(file: &DiffFile) -> String {
    if has_path_change_label(file) {
        format!("{} -> {}", basename(&file.old_path), basename(&file.path))
    } else {
        basename(file.display_path()).to_string()
    }
}

fn has_path_change_label(file: &DiffFile) -> bool {
    matches!(file.status, FileStatus::Renamed | FileStatus::Copied) && file.old_path != file.path
}

pub(super) fn file_status_suffix(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => " (new)",
        FileStatus::Deleted => " (deleted)",
        FileStatus::Copied => " (copied)",
        FileStatus::Renamed | FileStatus::Modified => "",
    }
}

/// Nerd Font glyph marking a file as reviewed in the sidebar.
pub(super) fn reviewed_glyph() -> &'static str {
    "\u{f00c}" // check
}

/// A file's language or type, inferred from its path. Drives both the sidebar
/// glyph and its color so the two stay in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Rust,
    TypeScript,
    React,
    JavaScript,
    Json,
    Python,
    Go,
    Ruby,
    Java,
    C,
    CHeader,
    Cpp,
    CSharp,
    Markdown,
    Html,
    Css,
    Sass,
    Vue,
    Shell,
    Config,
    Yaml,
    Text,
    Image,
    Lock,
    Generic,
}

impl FileKind {
    /// Nerd Font glyph hinting at the file's language or type.
    fn icon(self) -> &'static str {
        match self {
            Self::Rust => "\u{e7a8}",
            Self::TypeScript => "\u{e628}",
            Self::React => "\u{e7ba}",
            Self::JavaScript => "\u{e74e}",
            Self::Json => "\u{e60b}",
            Self::Python => "\u{e606}",
            Self::Go => "\u{e627}",
            Self::Ruby => "\u{e21e}",
            Self::Java => "\u{e256}",
            Self::C => "\u{e61e}",
            Self::CHeader => "\u{f0fd}",
            Self::Cpp => "\u{e61d}",
            Self::CSharp => "\u{e648}",
            Self::Markdown => "\u{e609}",
            Self::Html => "\u{e736}",
            Self::Css => "\u{e749}",
            Self::Sass => "\u{e603}",
            Self::Vue => "\u{fd42}",
            Self::Shell => "\u{e795}",
            Self::Config => "\u{f013}",
            Self::Yaml => "\u{e615}",
            Self::Text => "\u{f15c}",
            Self::Image => "\u{f1c5}",
            Self::Lock => "\u{f023}",
            Self::Generic => "\u{f15b}",
        }
    }

    /// Theme-aware color for the file-type icon. Colors are sourced from the
    /// active theme's syntax palette (plus a couple of base colors) so icons
    /// stay readable in every supported theme. Unknown/extensionless files and
    /// lock files use muted base colors as a deliberate fallback.
    fn color(self, theme: Theme) -> Color {
        let palette = theme.syntax;
        match self {
            Self::Rust => palette.keyword,
            Self::TypeScript => palette.property,
            Self::React => palette.support,
            Self::JavaScript => palette.type_name,
            Self::Json => palette.list_marker,
            Self::Python => palette.link,
            Self::Go => palette.tag,
            Self::Ruby => palette.invalid,
            Self::Java => palette.regex,
            Self::C => palette.property,
            Self::CHeader => palette.namespace,
            Self::Cpp => palette.markup,
            Self::CSharp => palette.function,
            Self::Markdown => palette.markup,
            Self::Html => palette.regex,
            Self::Css => palette.link,
            Self::Sass => palette.label,
            Self::Vue => palette.string,
            Self::Shell => palette.string,
            Self::Config => palette.comment,
            Self::Yaml => palette.constant,
            Self::Text => theme.text,
            Self::Image => palette.constant,
            Self::Lock => theme.muted,
            Self::Generic => theme.muted,
        }
    }
}

/// Classifies a file path into a [`FileKind`] from its name/extension.
fn file_kind(path: &str) -> FileKind {
    use FileKind::*;

    let lowercased = basename(path).to_ascii_lowercase();
    let extension = lowercased.rsplit_once('.').map(|(_, ext)| ext);

    match extension {
        Some("rs") => Rust,
        Some("ts") => TypeScript,
        Some("tsx" | "jsx") => React,
        Some("js" | "mjs" | "cjs") => JavaScript,
        Some("json") => Json,
        Some("py") => Python,
        Some("go") => Go,
        Some("rb") => Ruby,
        Some("java") => Java,
        Some("c") => C,
        Some("h" | "hpp" | "hh") => CHeader,
        Some("cpp" | "cc" | "cxx") => Cpp,
        Some("cs") => CSharp,
        Some("md" | "markdown") => Markdown,
        Some("html" | "htm") => Html,
        Some("css") => Css,
        Some("scss" | "sass") => Sass,
        Some("vue") => Vue,
        Some("sh" | "bash" | "zsh") => Shell,
        Some("toml" | "ini" | "cfg" | "conf") => Config,
        Some("yaml" | "yml") => Yaml,
        Some("txt") => Text,
        Some("png" | "jpg" | "jpeg" | "gif" | "svg" | "webp") => Image,
        Some("lock") => Lock,
        _ => Generic,
    }
}

/// Nerd Font glyph hinting at a file's language or type from its path.
pub(super) fn file_icon(path: &str) -> &'static str {
    file_kind(path).icon()
}

/// Theme-aware color for a file's type icon, inferred from its path.
pub(super) fn file_icon_color(path: &str, theme: Theme) -> Color {
    file_kind(path).color(theme)
}

pub(super) fn stage_display(stage: FileStage, background: Color, theme: Theme) -> StageDisplay {
    match stage {
        FileStage::Unstaged => StageDisplay {
            suffix: "  \u{f10c} unstaged",
            style: color_style(theme.muted, background),
        },
        FileStage::Staged => StageDisplay {
            suffix: "  \u{f058} staged",
            style: color_style(theme.added, background).add_modifier(Modifier::BOLD),
        },
        FileStage::Mixed => StageDisplay {
            suffix: "  \u{f056} mixed",
            style: color_style(theme.accent, background).add_modifier(Modifier::BOLD),
        },
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub(super) fn format_file_stats(file: &DiffFile) -> String {
    match (file.additions, file.deletions) {
        (0, 0) => String::new(),
        (additions, 0) => format!("+{additions}"),
        (0, deletions) => format!("-{deletions}"),
        (additions, deletions) => format!("+{additions} -{deletions}"),
    }
}

pub(super) fn padding_before_stats(
    content_width: usize,
    used_width: usize,
    stats_width: usize,
) -> String {
    if stats_width == 0 {
        String::new()
    } else {
        " ".repeat(content_width.saturating_sub(used_width).max(1))
    }
}

pub(super) fn stats_width(stats: &str) -> usize {
    display_width(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use FileKind::*;

    fn assert_file_kinds(cases: &[(&str, FileKind)]) {
        for &(path, kind) in cases {
            assert_eq!(file_kind(path), kind, "{path}");
        }
    }

    #[test]
    fn common_file_types_map_to_distinct_kinds() {
        assert_file_kinds(&[
            ("src/main.rs", Rust),
            ("app/index.ts", TypeScript),
            ("ui/Button.tsx", React),
            ("ui/Button.jsx", React),
            ("script.js", JavaScript),
            ("data.json", Json),
            ("run.py", Python),
            ("server.go", Go),
            ("README.md", Markdown),
            ("Cargo.toml", Config),
            ("config.yaml", Yaml),
            ("logo.png", Image),
        ]);
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_file_kinds(&[("PHOTO.PNG", Image), ("Main.RS", Rust)]);
    }

    #[test]
    fn lock_files_use_the_lock_kind_regardless_of_prefix() {
        assert_file_kinds(&[
            ("Cargo.lock", Lock),
            ("yarn.lock", Lock),
            ("deep/nested/poetry.lock", Lock),
        ]);
    }

    #[test]
    fn unknown_and_extensionless_files_fall_back_to_generic() {
        assert_file_kinds(&[
            ("Makefile", Generic),
            ("LICENSE", Generic),
            ("archive.unknownext", Generic),
            ("", Generic),
            (".gitignore", Generic),
        ]);
    }

    #[test]
    fn icon_matches_the_classified_kind() {
        assert_eq!(file_icon("src/main.rs"), Rust.icon());
        assert_eq!(file_icon("Makefile"), Generic.icon());
        assert_eq!(file_icon("Cargo.lock"), Lock.icon());
    }

    #[test]
    fn known_file_types_use_a_type_colored_icon_in_each_theme() {
        for theme in [Theme::gruvbox_dark_hard(), Theme::github_dark()] {
            // Rust and Markdown classify to different kinds, so their icon
            // colors must differ to make types scannable.
            assert_ne!(
                file_icon_color("src/main.rs", theme),
                file_icon_color("README.md", theme),
            );
        }
    }

    #[test]
    fn generic_and_lock_files_fall_back_to_muted_color() {
        for theme in [Theme::gruvbox_dark_hard(), Theme::github_dark()] {
            assert_eq!(file_icon_color("Makefile", theme), theme.muted);
            assert_eq!(file_icon_color("Cargo.lock", theme), theme.muted);
        }
    }
}
