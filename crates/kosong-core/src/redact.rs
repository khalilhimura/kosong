//! Credential redaction for child-process output.
//!
//! Applied to every byte of child output before it is stored, displayed, or
//! logged, because `gh` and `wrangler` can echo a token in an error.
//!
//! # What this deliberately does not do
//!
//! It does not redact every long random-looking string. A 40-character hex
//! run is far more likely to be a commit SHA than a secret, and blanking those
//! would make `git` and `gh` output useless while protecting nothing. The
//! approach is targeted: known credential prefixes, and values that follow a
//! key which names a credential.

/// Prefixes that identify a credential beyond reasonable doubt.
const SECRET_PREFIXES: [&str; 15] = [
    "ghp_",        // GitHub personal access token
    "gho_",        // GitHub OAuth token
    "ghu_",        // GitHub user-to-server token
    "ghs_",        // GitHub server-to-server token
    "ghr_",        // GitHub refresh token
    "github_pat_", // GitHub fine-grained token
    "glpat-",      // GitLab
    // Slack, spelled out by token type rather than as the three-character
    // `xox` this used to be. `xox` is a prefix of `xoxo`, which is an ordinary
    // thing to call a blog, and a Pages project named `xoxo-blog` was being
    // blanked out of `wrangler pages project list` as a result. Every Slack
    // token type carries the trailing letter and hyphen, so naming them costs
    // nothing and stops the collision at its source.
    "xoxa-",
    "xoxb-",
    "xoxc-",
    "xoxd-",
    "xoxe-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
];

/// Keys whose value is a credential, in `key=value` or `key: value` form.
const SECRET_KEYS: [&str; 8] = [
    "authorization",
    "bearer",
    "token",
    "api_token",
    "access_token",
    "refresh_token",
    "password",
    "secret",
];

/// The replacement written in place of a secret.
pub const REDACTED: &str = "[redacted]";

/// Removes anything recognisable as a credential from text.
pub fn redact(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // Two passes. The prefix pass catches a credential anywhere, including
    // inside JSON where there is no whitespace to split on. The key/value pass
    // catches secrets that have no recognisable shape of their own and are
    // identifiable only by the word next to them.
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        output.push_str(&redact_key_values(&redact_prefixes(line)));
    }
    output
}

/// Whether a character can be part of a credential.
fn is_secret_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Replaces anything starting with a known credential prefix.
///
/// Scans the whole string rather than whitespace-separated words, so a token
/// embedded in JSON such as `{"token":"ghp_..."}` is still caught. Punctuation
/// is what makes that work: `"`, `=`, `:` and whitespace all end a word, so a
/// credential wrapped in any of them still begins one.
///
/// # A credential begins a word
///
/// The scan starts at every byte, so without a boundary check a prefix landing
/// mid-word took the rest of the word with it — `the-ghp_naming-convention`
/// became `the-[redacted]`. Over-redaction is not a safe failure here: this
/// output is parsed as well as displayed, and `project_exists` reading a
/// blanked-out project name concludes the project is absent and has kosong
/// create one that already exists.
fn redact_prefixes(text: &str) -> String {
    let lowered = text.to_ascii_lowercase();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;

    while index < text.len() {
        // `index` is always a character boundary: the no-match arm below
        // advances by whole characters, and a prefix is only ever consumed
        // together with the credential body after it.
        let begins_a_word = text[..index]
            .chars()
            .next_back()
            .is_none_or(|c| !is_secret_char(c));

        let matched = SECRET_PREFIXES
            .iter()
            .filter(|_| begins_a_word)
            .find(|prefix| lowered[index..].starts_with(*prefix));

        match matched {
            Some(prefix) => {
                // Consume the prefix and the credential body that follows it.
                let mut end = index + prefix.len();
                while let Some(c) = text[end..].chars().next() {
                    if is_secret_char(c) {
                        end += c.len_utf8();
                    } else {
                        break;
                    }
                }
                output.push_str(REDACTED);
                index = end;
            }
            None => {
                let c = text[index..].chars().next().expect("in bounds");
                output.push(c);
                index += c.len_utf8();
            }
        }
    }
    output
}

/// Replaces values identified by a neighbouring or attached credential key.
///
/// Handles `token=abc`, `token: abc`, `Authorization: Bearer abc`, and
/// `--api-token abc`. The label may be attached to the value or be the
/// preceding word, so both are handled in one pass over the line.
fn redact_key_values(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut remainder = line;
    let mut previous_was_label = false;

    while !remainder.is_empty() {
        let boundary = remainder
            .find(char::is_whitespace)
            .unwrap_or(remainder.len());
        let (token, rest) = remainder.split_at(boundary);

        if previous_was_label && !token.is_empty() && token != REDACTED {
            if names_a_credential(token) {
                // `Authorization: Bearer abc` — the word after the label is
                // another label, so the secret is one further along. Treating
                // "Bearer" as the value would leave the real token exposed.
                output.push_str(token);
                previous_was_label = true;
            } else {
                // The whole word is the secret.
                output.push_str(REDACTED);
                previous_was_label = false;
            }
        } else {
            let (rendered, is_label) = redact_token(token);
            output.push_str(&rendered);
            previous_was_label = is_label;
        }

        let separator_end = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        output.push_str(&rest[..separator_end]);
        remainder = &rest[separator_end..];
    }
    output
}

/// Redacts an attached `key=value`, and reports whether the token is a bare
/// credential label whose value is the next word.
fn redact_token(token: &str) -> (String, bool) {
    if token.is_empty() {
        return (String::new(), false);
    }

    // A bare label: `Authorization:`, `Bearer`, `--api-token`.
    if names_a_credential(token) {
        return (token.to_owned(), true);
    }

    for separator in ['=', ':'] {
        if let Some((key, value)) = token.split_once(separator) {
            let trimmed_value = value.trim_matches(|c: char| "\"'".contains(c));
            if trimmed_value.is_empty() {
                continue;
            }
            if names_a_credential(key) {
                return (token.replace(trimmed_value, REDACTED), false);
            }
        }
    }
    (token.to_owned(), false)
}

/// Whether a word names a credential, ignoring decoration.
///
/// Strips quotes, braces, leading dashes, and a trailing colon, so
/// `{"api_token"`, `--api-token`, and `Authorization:` all reduce to the bare
/// key before comparison.
fn names_a_credential(word: &str) -> bool {
    let cleaned: String = word
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_ascii_lowercase()
        .replace('-', "_");

    if cleaned.is_empty() {
        return false;
    }
    SECRET_KEYS
        .iter()
        .any(|key| cleaned == key.replace('-', "_") || cleaned.ends_with(&format!("_{key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_prefix_is_redacted_anywhere_in_text() {
        assert!(redact("token ghp_abc123").contains(REDACTED));
    }

    #[test]
    fn a_token_in_json_is_redacted() {
        let result = redact(r#"{"token":"ghp_abc123"}"#);
        assert!(result.contains(REDACTED));
        assert!(!result.contains("ghp_"));
    }

    #[test]
    fn a_normal_word_is_not_redacted() {
        assert_eq!(redact("hello world"), "hello world");
    }

    #[test]
    fn a_key_value_pair_is_redacted() {
        let result = redact("token=ghp_abc123");
        assert!(result.contains(REDACTED));
    }

    #[test]
    fn a_key_value_pair_in_json_is_redacted() {
        let result = redact(r#"{"access_token":"abc123"}"#);
        assert!(result.contains(REDACTED));
    }

    #[test]
    fn the_slack_xox_prefix_is_only_redacted_when_it_begins_a_word() {
        // xoxo-blog was being blanked because "xox" matched mid-word.
        let result = redact("xoxo-blog is my blog");
        assert_eq!(result, "xoxo-blog is my blog");
        assert!(!result.contains(REDACTED), "xoxo is not a token prefix");

        // Actual Slack tokens should still be caught.
        let result = redact("token is xoxb-abc123");
        assert!(result.contains(REDACTED));
    }

    #[test]
    fn email_addresses_are_not_redacted() {
        // The `@` symbol in an email must not trigger a false positive on
        // the secret-key matcher. "password@github.com" isn't a credential
        // just because it contains the word "password".
        let result = redact("my email is user@example.com");
        assert_eq!(result, "my email is user@example.com");
    }
}
