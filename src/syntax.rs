//! Syntax highlighting adapter.
//!
//! This module isolates Syntect setup and scope-to-palette mapping from the UI.
//! Callers provide file paths and plain line content; the adapter returns
//! Ratatui spans.

use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use syntect::highlighting::{
    Color as SyntectColor, FontStyle, HighlightIterator, HighlightState, Highlighter,
    ScopeSelectors, Style as SyntectStyle, StyleModifier, Theme as SyntectTheme, ThemeItem,
    ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

use crate::theme::SyntaxPalette;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_no_newlines);

pub struct SyntaxHighlighter {
    engine: Option<SyntaxEngine>,
}

struct SyntaxEngine {
    theme: SyntectTheme,
    parse_state: ParseState,
    highlight_state: HighlightState,
}

impl SyntaxHighlighter {
    pub fn disabled() -> Self {
        Self { engine: None }
    }

    pub fn for_path(path: &str, palette: SyntaxPalette) -> Self {
        Self {
            engine: syntax_for_path(path).map(|syntax| SyntaxEngine::new(syntax, palette)),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.engine.is_some()
    }

    pub fn highlight_line(&mut self, content: &str, base_style: Style) -> Vec<Span<'static>> {
        let Some(engine) = self.engine.as_mut() else {
            return plain_line(content, base_style);
        };
        let Some(ranges) = engine.highlight_line(content) else {
            return plain_line(content, base_style);
        };

        ranges
            .into_iter()
            .map(|(style, text)| styled_token(text, style, base_style))
            .collect()
    }

    pub fn advance_line(&mut self, content: &str) {
        if let Some(engine) = self.engine.as_mut() {
            engine.advance_line(content);
        }
    }
}

impl SyntaxEngine {
    fn new(syntax: &SyntaxReference, palette: SyntaxPalette) -> Self {
        let theme = syntax_theme(palette);
        let highlight_state = {
            let highlighter = Highlighter::new(&theme);
            HighlightState::new(&highlighter, ScopeStack::new())
        };

        Self {
            theme,
            parse_state: ParseState::new(syntax),
            highlight_state,
        }
    }

    fn highlight_line<'a>(&mut self, content: &'a str) -> Option<Vec<(SyntectStyle, &'a str)>> {
        let ops = self.parse_state.parse_line(content, &SYNTAX_SET).ok()?;
        let highlighter = Highlighter::new(&self.theme);
        Some(
            HighlightIterator::new(&mut self.highlight_state, &ops, content, &highlighter)
                .collect(),
        )
    }

    fn advance_line(&mut self, content: &str) {
        let Some(ops) = self.parse_state.parse_line(content, &SYNTAX_SET).ok() else {
            return;
        };
        let highlighter = Highlighter::new(&self.theme);
        for _ in HighlightIterator::new(&mut self.highlight_state, &ops, content, &highlighter) {}
    }
}

fn plain_line(content: &str, style: Style) -> Vec<Span<'static>> {
    vec![Span::styled(content.to_owned(), style)]
}

fn styled_token(text: &str, style: SyntectStyle, base_style: Style) -> Span<'static> {
    Span::styled(text.to_owned(), token_style(style, base_style))
}

#[cfg(test)]
fn syntax_name_for_path(path: &str) -> Option<&'static str> {
    syntax_for_path(path).map(|syntax| syntax.name.as_str())
}

fn syntax_for_path(path: &str) -> Option<&'static SyntaxReference> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }

    syntax_for_path_parts(path).or_else(|| syntax_for_known_path(path))
}

fn syntax_for_path_parts(path: &str) -> Option<&'static SyntaxReference> {
    let path = Path::new(path);
    let file_name = path.file_name().and_then(|component| component.to_str())?;
    if let Some(syntax) = SYNTAX_SET.find_syntax_by_extension(file_name) {
        return Some(syntax);
    }

    let extension = path.extension().and_then(|component| component.to_str())?;
    SYNTAX_SET.find_syntax_by_extension(extension)
}

fn syntax_for_known_path(path_text: &str) -> Option<&'static SyntaxReference> {
    let path = Path::new(path_text);
    let file_name_lowercase = path
        .file_name()
        .and_then(|component| component.to_str())
        .unwrap_or(path_text)
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(|component| component.to_str())
        .map(str::to_ascii_lowercase);

    let file_name = file_name_lowercase.as_str();

    let is_container_build_file = matches_known_file(file_name, "dockerfile")
        || matches_known_file(file_name, "containerfile");
    if is_container_build_file {
        return find_by_extension_or_name("dockerfile", "Dockerfile");
    }

    if matches_known_file(file_name, "makefile") {
        return find_by_extension_or_name("makefile", "Makefile");
    }

    if matches_known_file(file_name, ".env") {
        return find_first_syntax(&[("env", "DotENV"), ("sh", "Bash")]);
    }

    let (syntax_extension, syntax_name) = match file_name {
        ".gitignore" | ".dockerignore" => ("gitignore", "Git Ignore"),
        ".editorconfig" | "cargo.lock" | "go.mod" | "go.sum" => return find_toml_syntax(),
        "package-lock.json" | "flake.lock" => ("json", "JSON"),
        "pnpm-lock.yaml" => ("yaml", "YAML"),
        "yarn.lock" => return find_first_syntax(&[("yaml", "YAML"), ("toml", "TOML")]),
        _ => return syntax_for_known_extension(extension.as_deref()),
    };

    find_by_extension_or_name(syntax_extension, syntax_name)
}

fn matches_known_file(file_name: &str, known_file_name: &str) -> bool {
    match file_name.strip_prefix(known_file_name) {
        Some("") => true,
        Some(suffix) => suffix.starts_with('.'),
        None => false,
    }
}

fn syntax_for_known_extension(extension: Option<&str>) -> Option<&'static SyntaxReference> {
    let (syntax_extension, syntax_name) = match extension? {
        "rs" => ("rs", "Rust"),
        "vue" => return find_first_syntax(&[("vue", "Vue Component"), ("vue", "Vue")]),
        "svelte" => ("svelte", "Svelte"),
        "js" | "jsx" | "mjs" | "cjs" => ("js", "JavaScript"),
        "ts" | "mts" | "cts" => {
            return find_first_syntax(&[("ts", "TypeScript"), ("js", "JavaScript")]);
        }
        "tsx" => {
            return find_first_syntax(&[
                ("tsx", "TypescriptReact"),
                ("ts", "TypeScript"),
                ("js", "JavaScript"),
            ]);
        }
        "json" | "jsonc" | "json5" => ("json", "JSON"),
        "md" | "markdown" | "mdx" => ("md", "Markdown"),
        "html" | "htm" => ("html", "HTML"),
        "xml" | "xhtml" | "svg" => ("xml", "XML"),
        "css" => ("css", "CSS"),
        "scss" => ("scss", "SCSS"),
        "sass" => {
            return find_first_syntax(&[("sass", "Sass"), ("scss", "SCSS"), ("css", "CSS")]);
        }
        "less" => ("less", "LESS"),
        "styl" | "stylus" => ("styl", "Stylus"),
        "yaml" | "yml" => ("yaml", "YAML"),
        "graphql" | "gql" => ("graphql", "GraphQL"),
        "sql" | "psql" | "mysql" => ("sql", "SQL"),
        "sh" | "bash" | "zsh" => ("sh", "ShellScript"),
        "fish" => ("fish", "Fish"),
        "ps1" => ("ps1", "PowerShell"),
        "toml" => return find_toml_syntax(),
        "ini" => ("ini", "INI"),
        "py" | "pyw" => ("py", "Python"),
        "go" => ("go", "Go"),
        "java" => ("java", "Java"),
        "kt" | "kts" => ("kt", "Kotlin"),
        "swift" => ("swift", "Swift"),
        "php" => ("php", "PHP"),
        "rb" => ("rb", "Ruby"),
        "lua" => ("lua", "Lua"),
        "vim" => ("vim", "VimL"),
        "nix" => ("nix", "Nix"),
        "tf" | "tfvars" => ("tf", "Terraform"),
        "c" | "h" => ("c", "C"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => ("cpp", "C++"),
        "cs" => ("cs", "C#"),
        _ => return None,
    };

    find_by_extension_or_name(syntax_extension, syntax_name)
}

fn find_toml_syntax() -> Option<&'static SyntaxReference> {
    find_first_syntax(&[
        ("toml", "TOML"),
        ("ini", "INI"),
        ("yaml", "YAML"),
        ("json", "JSON"),
    ])
}

fn find_first_syntax(candidates: &[(&str, &str)]) -> Option<&'static SyntaxReference> {
    candidates
        .iter()
        .find_map(|&(extension, name)| find_by_extension_or_name(extension, name))
}

fn find_by_extension_or_name(extension: &str, name: &str) -> Option<&'static SyntaxReference> {
    SYNTAX_SET
        .find_syntax_by_extension(extension)
        .or_else(|| SYNTAX_SET.find_syntax_by_name(name))
}

fn syntax_theme(palette: SyntaxPalette) -> SyntectTheme {
    let theme_color = |color| Some(syntect_color(color));

    SyntectTheme {
        name: Some("chunk".to_string()),
        author: Some("chunk".to_string()),
        settings: ThemeSettings {
            foreground: theme_color(palette.foreground),
            background: theme_color(palette.background),
            caret: theme_color(palette.foreground),
            accent: theme_color(palette.support),
            selection: theme_color(palette.selection),
            ..ThemeSettings::default()
        },
        scopes: vec![
            italic_theme_item("comment", palette.comment),
            italic_theme_item(
                "comment.line.documentation, comment.block.documentation",
                palette.doc_comment,
            ),
            theme_item("punctuation.definition.comment", palette.comment),
            theme_item("string", palette.string),
            theme_item("constant.character.escape", palette.escape),
            theme_item("constant.numeric", palette.constant),
            theme_item("constant.language", palette.constant),
            theme_item("constant.other, constant.character", palette.constant),
            theme_item("variable.language", palette.constant),
            theme_item("keyword", palette.keyword),
            theme_item("keyword.control, keyword.other", palette.keyword),
            theme_item("keyword.operator", palette.operator),
            theme_item("storage, storage.type, storage.modifier", palette.keyword),
            theme_item(
                "punctuation.separator, punctuation.terminator, punctuation.accessor",
                palette.punctuation,
            ),
            theme_item(
                "punctuation.section, punctuation.definition, punctuation.delimiter",
                palette.punctuation,
            ),
            theme_item(
                "support.type, entity.name.type, entity.name.class",
                palette.type_name,
            ),
            bold_theme_item("entity.name.function, support.function", palette.function),
            theme_item(
                "entity.name.macro, support.macro, meta.macro",
                palette.macro_call,
            ),
            theme_item(
                "entity.name.namespace, entity.name.module, entity.name.package",
                palette.namespace,
            ),
            theme_item(
                "variable.parameter, variable.other.member",
                palette.variable,
            ),
            theme_item(
                "variable.other.property, meta.property, support.type.property-name",
                palette.property,
            ),
            theme_item(
                "meta.object-literal.key, string.unquoted, entity.name.label",
                palette.label,
            ),
            theme_item("support.constant, support.variable", palette.support),
            theme_item("entity.name.tag, punctuation.definition.tag", palette.tag),
            theme_item("entity.other.attribute-name", palette.attribute),
            bold_theme_item("markup.heading", palette.markup),
            bold_theme_item("markup.bold", palette.markup),
            italic_theme_item("markup.italic", palette.constant),
            theme_item("markup.raw, markup.inline.raw", palette.string),
            italic_theme_item("markup.quote", palette.comment),
            theme_item(
                "markup.link, markup.underline.link, constant.other.reference.link",
                palette.link,
            ),
            theme_item(
                "markup.list, punctuation.definition.list",
                palette.list_marker,
            ),
            theme_item(
                "string.regexp, constant.other.character-class",
                palette.regex,
            ),
            theme_item("punctuation.definition.string", palette.string),
            theme_item("invalid", palette.invalid),
        ],
    }
}

fn theme_item(scope: &str, color: Color) -> ThemeItem {
    theme_item_with_font_style(scope, color, None)
}

fn italic_theme_item(scope: &str, color: Color) -> ThemeItem {
    theme_item_with_font_style(scope, color, Some(FontStyle::ITALIC))
}

fn bold_theme_item(scope: &str, color: Color) -> ThemeItem {
    theme_item_with_font_style(scope, color, Some(FontStyle::BOLD))
}

fn theme_item_with_font_style(
    scope: &str,
    color: Color,
    font_style: Option<FontStyle>,
) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(scope).expect("syntax scope selector should be valid"),
        style: StyleModifier {
            foreground: Some(syntect_color(color)),
            background: None,
            font_style,
        },
    }
}

fn syntect_color(color: Color) -> SyntectColor {
    let (r, g, b) = rgb(color);
    SyntectColor { r, g, b, a: 0xff }
}

fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Reset => (0xeb, 0xdb, 0xb2),
        Color::Black => (0x00, 0x00, 0x00),
        Color::Red => (0x80, 0x00, 0x00),
        Color::Green => (0x00, 0x80, 0x00),
        Color::Yellow => (0x80, 0x80, 0x00),
        Color::Blue => (0x00, 0x00, 0x80),
        Color::Magenta => (0x80, 0x00, 0x80),
        Color::Cyan => (0x00, 0x80, 0x80),
        Color::Gray => (0xc0, 0xc0, 0xc0),
        Color::DarkGray => (0x80, 0x80, 0x80),
        Color::LightRed => (0xff, 0x00, 0x00),
        Color::LightGreen => (0x00, 0xff, 0x00),
        Color::LightYellow => (0xff, 0xff, 0x00),
        Color::LightBlue => (0x00, 0x00, 0xff),
        Color::LightMagenta => (0xff, 0x00, 0xff),
        Color::LightCyan => (0x00, 0xff, 0xff),
        Color::White => (0xff, 0xff, 0xff),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => indexed_rgb(index),
    }
}

fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0x80, 0x00, 0x00),
        (0x00, 0x80, 0x00),
        (0x80, 0x80, 0x00),
        (0x00, 0x00, 0x80),
        (0x80, 0x00, 0x80),
        (0x00, 0x80, 0x80),
        (0xc0, 0xc0, 0xc0),
        (0x80, 0x80, 0x80),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x00, 0x00, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];

    match index {
        0..=15 => ANSI[usize::from(index)],
        16..=231 => {
            let value = index - 16;
            let r = xterm_level(value / 36);
            let g = xterm_level((value % 36) / 6);
            let b = xterm_level(value % 6);
            (r, g, b)
        }
        232..=255 => {
            let value = 8 + (index - 232) * 10;
            (value, value, value)
        }
    }
}

fn xterm_level(value: u8) -> u8 {
    if value == 0 { 0 } else { 55 + value * 40 }
}

fn token_style(style: SyntectStyle, base_style: Style) -> Style {
    let mut result_style = base_style.fg(ratatui_color(style.foreground));
    if style.font_style.contains(FontStyle::BOLD) {
        result_style = result_style.add_modifier(Modifier::BOLD);
    }

    result_style
}

fn ratatui_color(color: SyntectColor) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_project_languages() {
        for path in [
            "src/main.rs",
            "src/app.js",
            "src/app.ts",
            "src/app.tsx",
            "src/App.vue",
            "src/App.svelte",
            "src/styles.scss",
            "src/styles.sass",
            "src/styles.less",
            "src/schema.graphql",
            "package.json",
            "README.md",
            "docker/Dockerfile.dev",
            "docker/Containerfile",
            ".env.local",
            "config/app.yaml",
            "queries/report.sql",
            "scripts/task.py",
            "cmd/server.go",
            "src/Main.kt",
            "src/App.swift",
            "src/plugin.lua",
            "script.sh",
            "Cargo.toml",
            "go.mod",
            "go.sum",
            ".gitignore",
            ".dockerignore",
            ".editorconfig",
            "package-lock.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "flake.lock",
        ] {
            assert!(syntax_name_for_path(path).is_some(), "{path}");
        }
    }

    #[test]
    fn vue_components_use_theme_token_colors() {
        let palette = SyntaxPalette::github_dark_on_matte();
        let base_style = Style::default().fg(Color::White).bg(Color::Black);
        let mut highlighter = SyntaxHighlighter::for_path("src/App.vue", palette);

        let spans = highlighter.highlight_line("<template>", base_style);

        assert!(highlighter.is_enabled());
        assert!(spans.iter().any(|span| span.style.fg == Some(palette.tag)));
        assert!(spans.iter().all(|span| span.style.bg == Some(Color::Black)));
    }

    #[test]
    fn unknown_paths_fall_back_to_plain_spans() {
        let base_style = Style::default().fg(Color::White).bg(Color::Black);
        let mut highlighter = SyntaxHighlighter::for_path(
            "assets/blob.chunk-unknown",
            SyntaxPalette::github_dark_on_matte(),
        );

        let spans = highlighter.highlight_line("let value = 1;", base_style);

        assert!(!highlighter.is_enabled());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "let value = 1;");
        assert_eq!(spans[0].style, base_style);
    }

    #[test]
    fn recognized_paths_use_theme_token_colors() {
        let palette = SyntaxPalette::github_dark_on_matte();
        let base_style = Style::default().fg(Color::White).bg(Color::Black);
        let mut highlighter = SyntaxHighlighter::for_path("src/main.rs", palette);

        let spans = highlighter.highlight_line("fn main() { \"hi\" }", base_style);

        assert!(
            spans
                .iter()
                .any(|span| span.style.fg == Some(palette.keyword))
        );
        assert!(
            spans
                .iter()
                .any(|span| span.style.fg == Some(palette.string))
        );
        assert!(spans.iter().all(|span| span.style.bg == Some(Color::Black)));
    }

    #[test]
    fn maps_additional_token_classes_from_theme_palette() {
        let palette = SyntaxPalette {
            operator: Color::Rgb(1, 2, 3),
            punctuation: Color::Rgb(4, 5, 6),
            macro_call: Color::Rgb(7, 8, 9),
            property: Color::Rgb(10, 11, 12),
            ..SyntaxPalette::github_dark_on_matte()
        };
        let base_style = Style::default().fg(Color::White).bg(Color::Black);
        let mut highlighter = SyntaxHighlighter::for_path("src/main.rs", palette);

        let spans =
            highlighter.highlight_line("let value = foo::bar!(self.field + 1);", base_style);

        assert!(
            spans
                .iter()
                .any(|span| span.style.fg == Some(palette.operator))
        );
        assert!(
            spans
                .iter()
                .any(|span| span.style.fg == Some(palette.punctuation))
        );
    }
}
