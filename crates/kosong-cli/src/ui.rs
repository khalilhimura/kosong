//! Terminal output.
//!
//! Three rules from the product and non-functional requirements shape this:
//!
//! 1. **Plain language, never jargon.** "GitHub CLI is installed, but it is not
//!    signed in yet" — not "provider authentication precondition failed".
//! 2. **Never colour-only meaning.** Every status carries a symbol and a word,
//!    so the output survives `NO_COLOR`, a monochrome terminal, and a screen
//!    reader.
//! 3. **`--quiet` hides lessons and success prose, never errors.**
//!
//! Errors and warnings go to stderr; everything else to stdout, so `--json`
//! output stays pipeable.

use std::io::{self, IsTerminal, Write};

/// Status of one reported thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Good,
    Warn,
    Bad,
    Info,
}

impl Mark {
    /// A symbol that means something on its own, without colour.
    fn symbol(self) -> &'static str {
        match self {
            Self::Good => "✔",
            Self::Warn => "!",
            Self::Bad => "✖",
            Self::Info => "·",
        }
    }

    fn colour(self) -> &'static str {
        match self {
            Self::Good => "\x1b[32m",
            Self::Warn => "\x1b[33m",
            Self::Bad => "\x1b[31m",
            Self::Info => "\x1b[90m",
        }
    }
}

/// Column width for field labels, sized to the longest label in use.
const FIELD_WIDTH: usize = 11;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[90m";

/// Terminal writer honouring `--quiet`, `NO_COLOR`, and whether output is a pipe.
#[derive(Debug, Clone, Copy)]
pub struct Ui {
    quiet: bool,
    colour: bool,
}

impl Ui {
    /// Builds a writer for the current environment.
    ///
    /// Colour is disabled when `NO_COLOR` is set to any value, when `TERM` is
    /// `dumb`, or when stdout is not a terminal.
    pub fn new(quiet: bool) -> Self {
        Self {
            quiet,
            colour: colour_supported(),
        }
    }

    /// A writer that never emits colour. Used by tests and `--json`.
    pub fn plain(quiet: bool) -> Self {
        Self {
            quiet,
            colour: false,
        }
    }

    pub fn is_quiet(self) -> bool {
        self.quiet
    }

    fn paint(self, colour: &str, text: &str) -> String {
        if self.colour {
            format!("{colour}{text}{RESET}")
        } else {
            text.to_owned()
        }
    }

    /// A blank line, suppressed when quiet.
    pub fn blank(self) {
        if !self.quiet {
            println!();
        }
    }

    /// Ordinary output. Suppressed when quiet.
    pub fn say(self, text: impl AsRef<str>) {
        if !self.quiet {
            println!("{}", text.as_ref());
        }
    }

    /// Output that survives `--quiet`, for things the user explicitly asked to see.
    pub fn always(self, text: impl AsRef<str>) {
        println!("{}", text.as_ref());
    }

    /// A headline.
    pub fn heading(self, text: impl AsRef<str>) {
        if !self.quiet {
            println!("{}", self.paint(BOLD, text.as_ref()));
        }
    }

    /// A status line: symbol, then label, then detail.
    pub fn status(self, mark: Mark, label: impl AsRef<str>, detail: impl AsRef<str>) {
        let detail = detail.as_ref();
        let line = if detail.is_empty() {
            format!("{} {}", mark.symbol(), label.as_ref())
        } else {
            format!("{} {}  {}", mark.symbol(), label.as_ref(), detail)
        };
        println!("{}", self.paint(mark.colour(), &line));
    }

    /// A `name  value` pair, with names padded into a column.
    ///
    /// Padding happens here rather than at the call sites, so adding a longer
    /// label cannot silently misalign a whole block of output.
    pub fn field(self, name: &str, value: impl AsRef<str>) {
        if !self.quiet {
            println!("  {name:<FIELD_WIDTH$}  {}", value.as_ref());
        }
    }

    /// The one idea a command teaches. Suppressed when quiet.
    pub fn lesson(self, text: impl AsRef<str>) {
        if !self.quiet {
            println!("{}", self.paint(DIM, text.as_ref()));
        }
    }

    /// The next command to run. This is the spine of the onboarding ladder,
    /// so it is always shown with the literal text the user should type.
    pub fn next_command(self, command: &str) {
        if !self.quiet {
            println!();
            println!("Next: {}", self.paint(BOLD, command));
        }
    }

    /// A warning. Goes to stderr and survives `--quiet`.
    pub fn warn(self, text: impl AsRef<str>) {
        let line = format!("{} {}", Mark::Warn.symbol(), text.as_ref());
        eprintln!("{}", self.paint(Mark::Warn.colour(), &line));
    }

    /// An error plus its repair action. Goes to stderr and survives `--quiet`.
    pub fn error(self, message: &str, repair: Option<&str>) {
        let line = format!("{} {message}", Mark::Bad.symbol());
        eprintln!("{}", self.paint(Mark::Bad.colour(), &line));

        if let Some(repair) = repair {
            eprintln!();
            for line in repair.lines() {
                eprintln!("{line}");
            }
        }
    }

    /// Asks a yes/no question, defaulting to `default` when input is not a terminal.
    ///
    /// Automation must not hang waiting for a prompt, so a non-interactive
    /// stdin takes the default rather than blocking.
    pub fn confirm(self, question: &str, default: bool) -> bool {
        if !io::stdin().is_terminal() {
            return default;
        }

        let hint = if default { "[Y/n]" } else { "[y/N]" };
        print!("{question} {hint} ");
        if io::stdout().flush().is_err() {
            return default;
        }

        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err() {
            return default;
        }

        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => true,
            "n" | "no" => false,
            _ => default,
        }
    }

    /// Asks for a line of text, returning `None` when input is not a terminal
    /// or the user entered nothing.
    pub fn ask(self, question: &str) -> Option<String> {
        if !io::stdin().is_terminal() {
            return None;
        }

        print!("{question} ");
        if io::stdout().flush().is_err() {
            return None;
        }

        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err() {
            return None;
        }

        let trimmed = answer.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }
}

/// Whether to emit ANSI colour.
///
/// Follows the `NO_COLOR` convention: the variable's presence disables colour
/// regardless of its value.
fn colour_supported() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var("TERM").map(|t| t == "dumb").unwrap_or(false) {
        return false;
    }
    io::stdout().is_terminal()
}
