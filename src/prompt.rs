//! The questions `rosco init` asks when it has a terminal to ask them in.
//!
//! Everything here has an option on the command line as well, and a default
//! that a non-interactive run takes without asking, so a script never waits
//! for an answer that is not coming.

use std::io::{IsTerminal, Write};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::{cursor, queue, terminal};

/// One answer to a question, with a word about what choosing it means.
///
/// The two are borrowed rather than owned so a question can be built out of
/// what a previous run left behind as easily as out of constants.
#[derive(Clone, Copy, Debug)]
pub struct Choice<'a> {
    pub label: &'a str,
    pub detail: &'a str,
}

/// Whether questions can be asked at all: both ends of the conversation have
/// to be a terminal, or the answers would go into a pipe unseen.
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Prints the heading a run of questions belongs to.
pub fn heading(text: &str) {
    let mut out = std::io::stderr();
    let _ = queue!(
        out,
        SetAttribute(Attribute::Bold),
        Print(format!("\n{text}\n\n")),
        SetAttribute(Attribute::Reset),
    );
    let _ = out.flush();
}

/// Asks for a line of text, offering `default` for an empty answer.
pub fn text(question: &str, default: &str) -> Result<String> {
    let _raw = RawMode::enter()?;
    let mut answer = String::new();

    loop {
        draw_question(question, &answer, default)?;
        match key()? {
            KeyCode::Enter => break,
            KeyCode::Backspace => {
                answer.pop();
            }
            KeyCode::Char(character) => answer.push(character),
            _ => {}
        }
    }

    let answer = if answer.trim().is_empty() {
        default.to_string()
    } else {
        answer.trim().to_string()
    };
    clear_line()?;
    answered(question, &answer)?;
    Ok(answer)
}

/// Asks which of `choices` to use, starting on `default`.
pub fn select(question: &str, choices: &[Choice<'_>], default: usize) -> Result<usize> {
    let _raw = RawMode::enter()?;
    let mut current = default.min(choices.len().saturating_sub(1));

    loop {
        draw_choices(question, choices, current)?;
        match key()? {
            KeyCode::Enter | KeyCode::Char(' ') => break,
            KeyCode::Up | KeyCode::Char('k') => {
                current = current.checked_sub(1).unwrap_or(choices.len() - 1);
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                current = (current + 1) % choices.len();
            }
            _ => {}
        }
        // Back to the top of the block, ready to draw over it.
        let mut out = std::io::stderr();
        queue!(out, cursor::MoveUp(choices.len() as u16 + 1))
            .context("could not redraw the question")?;
        out.flush().context("could not redraw the question")?;
    }

    let mut out = std::io::stderr();
    queue!(
        out,
        cursor::MoveUp(choices.len() as u16 + 1),
        terminal::Clear(terminal::ClearType::FromCursorDown),
    )
    .context("could not redraw the question")?;
    out.flush().ok();
    answered(question, choices[current].label)?;
    Ok(current)
}

fn draw_question(question: &str, answer: &str, default: &str) -> Result<()> {
    let mut out = std::io::stderr();
    queue!(
        out,
        Print("\r"),
        terminal::Clear(terminal::ClearType::CurrentLine),
        SetForegroundColor(Color::Cyan),
        Print("? "),
        ResetColor,
        SetAttribute(Attribute::Bold),
        Print(question),
        SetAttribute(Attribute::Reset),
        Print(" › "),
    )
    .context("could not draw the question")?;
    if answer.is_empty() {
        queue!(
            out,
            SetAttribute(Attribute::Dim),
            Print(default),
            SetAttribute(Attribute::Reset),
        )
        .context("could not draw the question")?;
    } else {
        queue!(out, Print(answer)).context("could not draw the question")?;
    }
    out.flush().context("could not draw the question")
}

fn draw_choices(question: &str, choices: &[Choice<'_>], current: usize) -> Result<()> {
    let mut out = std::io::stderr();
    queue!(
        out,
        Print("\r"),
        terminal::Clear(terminal::ClearType::CurrentLine),
        SetForegroundColor(Color::Cyan),
        Print("? "),
        ResetColor,
        SetAttribute(Attribute::Bold),
        Print(question),
        SetAttribute(Attribute::Reset),
        Print("\r\n"),
    )
    .context("could not draw the question")?;

    let width = choices
        .iter()
        .map(|choice| choice.label.len())
        .max()
        .unwrap_or(0);
    // The block is redrawn by moving up a fixed number of lines, so a detail
    // long enough to wrap would leave the question behind on the screen.
    let room = terminal::size()
        .map(|(columns, _)| usize::from(columns))
        .unwrap_or(80)
        .saturating_sub(MARKER.len() + width + DETAIL_GAP.len() + 1);
    for (index, choice) in choices.iter().enumerate() {
        let selected = index == current;
        queue!(
            out,
            terminal::Clear(terminal::ClearType::CurrentLine),
            SetForegroundColor(if selected { Color::Cyan } else { Color::Reset }),
            Print(if selected { MARKER } else { "    " }),
            Print(format!("{:width$}", choice.label)),
            ResetColor,
            SetAttribute(Attribute::Dim),
            Print(format!("{DETAIL_GAP}{}", fit(choice.detail, room))),
            SetAttribute(Attribute::Reset),
            Print("\r\n"),
        )
        .context("could not draw the question")?;
    }
    out.flush().context("could not draw the question")
}

const MARKER: &str = "  > ";
const DETAIL_GAP: &str = "  ";

/// `text` cut to `width` columns, with an ellipsis where it was cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(3)).collect();
    format!("{}...", kept.trim_end())
}

fn answered(question: &str, answer: &str) -> Result<()> {
    let mut out = std::io::stderr();
    queue!(
        out,
        Print("\r"),
        terminal::Clear(terminal::ClearType::CurrentLine),
        SetForegroundColor(Color::Green),
        Print("+ "),
        ResetColor,
        Print(format!("{question}: ")),
        SetAttribute(Attribute::Bold),
        Print(answer),
        SetAttribute(Attribute::Reset),
        Print("\r\n"),
    )
    .context("could not draw the answer")?;
    out.flush().context("could not draw the answer")
}

fn clear_line() -> Result<()> {
    let mut out = std::io::stderr();
    queue!(
        out,
        Print("\r"),
        terminal::Clear(terminal::ClearType::CurrentLine)
    )
    .context("could not redraw the question")?;
    out.flush().context("could not redraw the question")
}

/// The next key pressed. Ctrl-C and Escape end the run rather than answering
/// something the caller did not mean.
fn key() -> Result<KeyCode> {
    loop {
        let Event::Key(key) = event::read().context("could not read from the terminal")? else {
            continue;
        };
        // Windows reports the release of a key as well as its press.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.code == KeyCode::Esc
            || (key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d')))
        {
            // The terminal has to be its old self again before anything is
            // printed about why we stopped.
            let _ = terminal::disable_raw_mode();
            eprintln!();
            bail!("cancelled");
        }

        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(key.code);
        }
        // A terminal that sends a line feed for Enter is indistinguishable
        // from one whose user held Ctrl and pressed J; either way the answer
        // is finished. Every other combination is for something we do not do,
        // and typing its letter into the answer would be worse than ignoring
        // it.
        if matches!(key.code, KeyCode::Char('j') | KeyCode::Char('m')) {
            return Ok(KeyCode::Enter);
        }
    }
}

struct RawMode;

impl RawMode {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("could not enable terminal raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detail_that_fits_is_left_alone() {
        assert_eq!(fit("the toolchain image", 40), "the toolchain image");
    }

    #[test]
    fn a_detail_that_does_not_is_cut_where_the_line_ends() {
        let cut = fit("rosco_6502/asm, host build, emulator, docker emulator", 20);
        assert_eq!(cut.chars().count(), 20);
        assert!(cut.ends_with("..."), "{cut}");
    }

    #[test]
    fn a_terminal_too_narrow_for_anything_still_produces_a_line() {
        assert_eq!(fit("anything", 0), "...");
    }
}
