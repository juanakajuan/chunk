//! UI and syntax color palettes.

use ratatui::style::{Color, Style};

const NIGHT_BLACK: Color = Color::Rgb(0x0b, 0x0b, 0x0a);
const CHARCOAL: Color = Color::Rgb(0x33, 0x33, 0x33);
const DARK_CHARCOAL: Color = Color::Rgb(0x24, 0x24, 0x24);
const DEEP_CHARCOAL: Color = Color::Rgb(0x16, 0x16, 0x14);
const GITHUB_DARK_ADDED_BG: Color = Color::Rgb(0x0f, 0x2a, 0x1a);
const GITHUB_DARK_REMOVED_BG: Color = Color::Rgb(0x2a, 0x12, 0x16);
const GITHUB_DARK_FG: Color = Color::Rgb(0xc9, 0xd1, 0xd9);
const GITHUB_DARK_MUTED: Color = Color::Rgb(0x8b, 0x94, 0x9e);
const GITHUB_DARK_BORDER: Color = Color::Rgb(0x30, 0x36, 0x3d);
const GITHUB_DARK_BLUE: Color = Color::Rgb(0x58, 0xa6, 0xff);
const GITHUB_DARK_LIGHT_BLUE: Color = Color::Rgb(0x79, 0xc0, 0xff);
const GITHUB_DARK_STRING: Color = Color::Rgb(0xa5, 0xd6, 0xff);
const GITHUB_DARK_GREEN: Color = Color::Rgb(0x3f, 0xb9, 0x50);
const GITHUB_DARK_RED: Color = Color::Rgb(0xf8, 0x51, 0x49);
const GITHUB_DARK_KEYWORD: Color = Color::Rgb(0xff, 0x7b, 0x72);
const GITHUB_DARK_ORANGE: Color = Color::Rgb(0xd2, 0x99, 0x22);
const GITHUB_DARK_BRIGHT_ORANGE: Color = Color::Rgb(0xff, 0xa6, 0x57);
const GITHUB_DARK_PURPLE: Color = Color::Rgb(0xd2, 0xa8, 0xff);
const GRUVBOX_HARD_BG: Color = Color::Rgb(0x1d, 0x20, 0x21);
const GRUVBOX_PANEL_BG: Color = Color::Rgb(0x1f, 0x1f, 0x1f);
const GRUVBOX_DARK_BG: Color = Color::Rgb(0x28, 0x28, 0x28);
const GRUVBOX_BG3: Color = Color::Rgb(0x66, 0x5c, 0x54);
const GRUVBOX_BG4: Color = Color::Rgb(0x7c, 0x6f, 0x64);
const GRUVBOX_ADDED_BG: Color = Color::Rgb(0x26, 0x33, 0x1d);
const GRUVBOX_REMOVED_BG: Color = Color::Rgb(0x3a, 0x21, 0x22);
const GRUVBOX_LIGHT_FG: Color = Color::Rgb(0xeb, 0xdb, 0xb2);
const GRUVBOX_GRAY: Color = Color::Rgb(0x92, 0x83, 0x74);
const GRUVBOX_RED: Color = Color::Rgb(0xfb, 0x49, 0x34);
const GRUVBOX_GREEN: Color = Color::Rgb(0xb8, 0xbb, 0x26);
const GRUVBOX_YELLOW: Color = Color::Rgb(0xfa, 0xbd, 0x2f);
const GRUVBOX_BLUE: Color = Color::Rgb(0x83, 0xa5, 0x98);
const GRUVBOX_PURPLE: Color = Color::Rgb(0xd3, 0x86, 0x9b);
const GRUVBOX_AQUA: Color = Color::Rgb(0x8e, 0xc0, 0x7c);
const GRUVBOX_ORANGE: Color = Color::Rgb(0xfe, 0x80, 0x19);
const GRUVBOX_SELECTION: Color = Color::Rgb(0x50, 0x49, 0x45);

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Color,
    pub background_alt: Color,
    pub border: Color,
    pub border_active: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    /// Dark ink color for text drawn on top of `accent` fills.
    pub on_accent: Color,
    pub added: Color,
    pub added_bg: Color,
    pub removed: Color,
    pub removed_bg: Color,
    pub selected: Color,
    pub file_new: Color,
    pub file_deleted: Color,
    pub file_renamed: Color,
    pub file_modified: Color,
    pub line_number_fg: Color,
    pub line_number_bg: Color,
    pub context_bg: Color,
    pub syntax: SyntaxPalette,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ThemeName {
    #[default]
    Gruvbox,
    GithubDark,
}

impl ThemeName {
    pub(crate) const CONFIG_VALUES: &'static [&'static str] = &["gruvbox", "github-dark"];

    pub(crate) fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "gruvbox" => Some(Self::Gruvbox),
            "github-dark" => Some(Self::GithubDark),
            _ => None,
        }
    }

    pub(crate) fn theme(self) -> Theme {
        match self {
            Self::Gruvbox => Theme::gruvbox_dark_hard(),
            Self::GithubDark => Theme::github_dark(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxPalette {
    pub background: Color,
    pub foreground: Color,
    pub selection: Color,
    pub comment: Color,
    pub string: Color,
    pub escape: Color,
    pub constant: Color,
    pub keyword: Color,
    pub type_name: Color,
    pub function: Color,
    pub variable: Color,
    pub support: Color,
    pub tag: Color,
    pub attribute: Color,
    pub markup: Color,
    pub invalid: Color,
    pub operator: Color,
    pub punctuation: Color,
    pub namespace: Color,
    pub property: Color,
    pub macro_call: Color,
    pub label: Color,
    pub regex: Color,
    pub link: Color,
    pub doc_comment: Color,
    pub list_marker: Color,
}

impl SyntaxPalette {
    pub fn gruvbox_dark() -> Self {
        Self {
            background: GRUVBOX_HARD_BG,
            foreground: GRUVBOX_LIGHT_FG,
            selection: GRUVBOX_SELECTION,
            comment: GRUVBOX_GRAY,
            string: GRUVBOX_GREEN,
            escape: GRUVBOX_ORANGE,
            constant: GRUVBOX_PURPLE,
            keyword: GRUVBOX_RED,
            type_name: GRUVBOX_YELLOW,
            function: GRUVBOX_GREEN,
            variable: GRUVBOX_BLUE,
            support: GRUVBOX_AQUA,
            tag: GRUVBOX_BLUE,
            attribute: GRUVBOX_AQUA,
            markup: GRUVBOX_YELLOW,
            invalid: GRUVBOX_RED,
            operator: GRUVBOX_ORANGE,
            punctuation: GRUVBOX_LIGHT_FG,
            namespace: GRUVBOX_BLUE,
            property: GRUVBOX_BLUE,
            macro_call: GRUVBOX_AQUA,
            label: GRUVBOX_PURPLE,
            regex: GRUVBOX_ORANGE,
            link: GRUVBOX_AQUA,
            doc_comment: GRUVBOX_GRAY,
            list_marker: GRUVBOX_ORANGE,
        }
    }

    pub fn github_dark_on_matte() -> Self {
        Self {
            background: NIGHT_BLACK,
            foreground: GITHUB_DARK_FG,
            selection: CHARCOAL,
            comment: GITHUB_DARK_MUTED,
            string: GITHUB_DARK_STRING,
            escape: GITHUB_DARK_BRIGHT_ORANGE,
            constant: GITHUB_DARK_PURPLE,
            keyword: GITHUB_DARK_KEYWORD,
            type_name: GITHUB_DARK_ORANGE,
            function: GITHUB_DARK_ORANGE,
            variable: GITHUB_DARK_FG,
            support: GITHUB_DARK_ORANGE,
            tag: GITHUB_DARK_GREEN,
            attribute: GITHUB_DARK_PURPLE,
            markup: GITHUB_DARK_BLUE,
            invalid: GITHUB_DARK_RED,
            operator: GITHUB_DARK_KEYWORD,
            punctuation: GITHUB_DARK_FG,
            namespace: GITHUB_DARK_FG,
            property: GITHUB_DARK_LIGHT_BLUE,
            macro_call: GITHUB_DARK_PURPLE,
            label: GITHUB_DARK_ORANGE,
            regex: GITHUB_DARK_BRIGHT_ORANGE,
            link: GITHUB_DARK_BLUE,
            doc_comment: GITHUB_DARK_MUTED,
            list_marker: GITHUB_DARK_BRIGHT_ORANGE,
        }
    }
}

impl Theme {
    pub fn gruvbox_dark_hard() -> Self {
        Self {
            background: Color::Reset,
            background_alt: GRUVBOX_PANEL_BG,
            border: GRUVBOX_BG3,
            border_active: GRUVBOX_BLUE,
            text: GRUVBOX_LIGHT_FG,
            muted: GRUVBOX_GRAY,
            accent: GRUVBOX_BLUE,
            on_accent: GRUVBOX_HARD_BG,
            added: GRUVBOX_GREEN,
            added_bg: GRUVBOX_ADDED_BG,
            removed: GRUVBOX_RED,
            removed_bg: GRUVBOX_REMOVED_BG,
            selected: GRUVBOX_SELECTION,
            file_new: GRUVBOX_GREEN,
            file_deleted: GRUVBOX_RED,
            file_renamed: GRUVBOX_YELLOW,
            file_modified: GRUVBOX_BLUE,
            line_number_fg: GRUVBOX_BG4,
            line_number_bg: GRUVBOX_DARK_BG,
            context_bg: Color::Reset,
            syntax: SyntaxPalette::gruvbox_dark(),
        }
    }

    pub fn github_dark() -> Self {
        Self {
            background: NIGHT_BLACK,
            background_alt: DARK_CHARCOAL,
            border: GITHUB_DARK_BORDER,
            border_active: GITHUB_DARK_BLUE,
            text: GITHUB_DARK_FG,
            muted: GITHUB_DARK_MUTED,
            accent: GITHUB_DARK_BLUE,
            on_accent: NIGHT_BLACK,
            added: GITHUB_DARK_GREEN,
            added_bg: GITHUB_DARK_ADDED_BG,
            removed: GITHUB_DARK_RED,
            removed_bg: GITHUB_DARK_REMOVED_BG,
            selected: CHARCOAL,
            file_new: GITHUB_DARK_GREEN,
            file_deleted: GITHUB_DARK_RED,
            file_renamed: GITHUB_DARK_ORANGE,
            file_modified: GITHUB_DARK_BLUE,
            line_number_fg: GITHUB_DARK_MUTED,
            line_number_bg: DEEP_CHARCOAL,
            context_bg: NIGHT_BLACK,
            syntax: SyntaxPalette::github_dark_on_matte(),
        }
    }

    pub fn base_style(self) -> Style {
        Style::default().fg(self.text).bg(self.background)
    }
}
