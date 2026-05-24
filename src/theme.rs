use ratatui::style::{Color, Style};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub background: Color,
    pub panel: Color,
    pub panel_alt: Color,
    pub border: Color,
    pub border_active: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub added: Color,
    pub added_bg: Color,
    pub removed: Color,
    pub removed_bg: Color,
    pub warning: Color,
    pub selected: Color,
}

impl Theme {
    pub fn matte_box() -> Self {
        Self {
            background: rgb(0x0b, 0x0b, 0x0a),
            panel: rgb(0x16, 0x16, 0x14),
            panel_alt: rgb(0x33, 0x33, 0x33),
            border: rgb(0x51, 0x51, 0x51),
            border_active: rgb(0x7d, 0xae, 0xa3),
            text: rgb(0xe2, 0xd2, 0xab),
            muted: rgb(0x8a, 0x8a, 0x8d),
            accent: rgb(0x7d, 0xae, 0xa3),
            added: rgb(0xff, 0xc1, 0x07),
            added_bg: rgb(0x24, 0x22, 0x12),
            removed: rgb(0xd3, 0x5f, 0x5f),
            removed_bg: rgb(0x26, 0x13, 0x13),
            warning: rgb(0xf5, 0x9e, 0x0b),
            selected: rgb(0x33, 0x33, 0x33),
        }
    }

    pub fn base_style(self) -> Style {
        Style::default().fg(self.text).bg(self.background)
    }
}

fn rgb(red: u8, green: u8, blue: u8) -> Color {
    Color::Rgb(red, green, blue)
}
