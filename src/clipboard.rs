//! OS clipboard adapter.

use color_eyre::eyre::Result;

pub(crate) fn write_text(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}
