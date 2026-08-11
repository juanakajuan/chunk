//! OS clipboard adapter.

use std::io::{self, Write};

use color_eyre::eyre::{Result, WrapErr};
use crossterm::clipboard::CopyToClipboard;
use crossterm::execute;

pub(crate) fn write_text(text: &str) -> Result<()> {
    write_text_with_fallback(text, write_graphical_text, || {
        write_terminal_text(io::stdout(), text)
    })
}

fn write_text_with_fallback(
    text: &str,
    write_graphical: impl FnOnce(&str) -> Result<()>,
    write_terminal: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let Err(graphical_error) = write_graphical(text) else {
        return Ok(());
    };

    write_terminal().wrap_err_with(|| {
        format!("graphical clipboard failed: {graphical_error}; terminal clipboard fallback failed")
    })
}

fn write_graphical_text(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    Ok(())
}

fn write_terminal_text(mut writer: impl Write, text: &str) -> Result<()> {
    execute!(writer, CopyToClipboard::to_clipboard_from(text))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::{Result, eyre};

    use super::{write_terminal_text, write_text, write_text_with_fallback};

    #[test]
    fn falls_back_to_terminal_when_graphical_clipboard_fails() {
        let mut terminal_called = false;

        write_text_with_fallback(
            "copied over ssh",
            |_| {
                Err(eyre!(
                    "X11 server connection timed out because it was unreachable"
                ))
            },
            || {
                terminal_called = true;
                Ok(())
            },
        )
        .expect("terminal clipboard fallback should succeed");

        assert!(terminal_called);
    }

    #[test]
    fn terminal_clipboard_uses_osc_52() -> Result<()> {
        let mut output = Vec::new();

        write_terminal_text(&mut output, "copied over ssh")?;

        assert_eq!(output, b"\x1b]52;c;Y29waWVkIG92ZXIgc3No\x1b\\");
        Ok(())
    }

    #[test]
    #[ignore = "requires deliberately unreachable graphical display sockets"]
    fn writes_text_when_graphical_clipboard_is_unreachable() {
        write_text("copied over ssh").expect("terminal clipboard fallback should succeed");
    }
}
