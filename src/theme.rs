use ratatui::style::{Color, Style};

const NIGHT_BLACK: Color = Color::Rgb(0x0b, 0x0b, 0x0a);
const CHARCOAL: Color = Color::Rgb(0x33, 0x33, 0x33);
const SLATE_GRAY: Color = Color::Rgb(0x51, 0x51, 0x51);
const SAGE: Color = Color::Rgb(0x7d, 0xae, 0xa3);
const SAND: Color = Color::Rgb(0xe2, 0xd2, 0xab);
const MIST_BLUE: Color = Color::Rgb(0x8b, 0x9b, 0xaa);
const MINT_GREEN: Color = Color::Rgb(0x6a, 0xd1, 0x8f);
const DARK_OLIVE: Color = Color::Rgb(0x24, 0x22, 0x12);
const DUSTY_RED: Color = Color::Rgb(0xd3, 0x5f, 0x5f);
const DARK_MAROON: Color = Color::Rgb(0x26, 0x13, 0x13);
const AMBER: Color = Color::Rgb(0xf5, 0x9e, 0x0b);
const COOL_GRAY: Color = Color::Rgb(0x8a, 0x8a, 0x8d);
const DEEP_CHARCOAL: Color = Color::Rgb(0x16, 0x16, 0x14);

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Color,
    pub background_alt: Color,
    pub border: Color,
    pub border_active: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
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
}

impl Theme {
    pub fn matte_box() -> Self {
        Self {
            background: NIGHT_BLACK,
            background_alt: CHARCOAL,
            border: SLATE_GRAY,
            border_active: SAGE,
            text: SAND,
            muted: MIST_BLUE,
            accent: SAGE,
            added: MINT_GREEN,
            added_bg: DARK_OLIVE,
            removed: DUSTY_RED,
            removed_bg: DARK_MAROON,
            selected: CHARCOAL,
            file_new: MINT_GREEN,
            file_deleted: DUSTY_RED,
            file_renamed: AMBER,
            file_modified: SAGE,
            line_number_fg: COOL_GRAY,
            line_number_bg: DEEP_CHARCOAL,
            context_bg: NIGHT_BLACK,
        }
    }

    pub fn base_style(self) -> Style {
        Style::default().fg(self.text).bg(self.background)
    }
}
