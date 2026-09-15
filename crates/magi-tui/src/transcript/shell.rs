//! A shell command, highlighted: the program, its subcommand, flags, paths, numbers, strings,
//! variables, keywords and the operators between commands each in their own colour, so a block
//! reads as code rather than as a grey line of argument text.

use super::Detail;
use crate::colour;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// What starts an operator, and what ends a word.
const OPERATORS: &str = "|&;<>()`";
const STOPS: &str = "|&;<>()`'\"$";

/// Programs whose first plain argument is a subcommand: `git commit`, `cargo build`.
const SUBCOMMANDED: &[&str] = &[
    "git",
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "deno",
    "docker",
    "podman",
    "kubectl",
    "helm",
    "systemctl",
    "apt",
    "apt-get",
    "dnf",
    "pacman",
    "brew",
    "pip",
    "pip3",
    "uv",
    "go",
    "rustup",
    "nix",
    "gh",
    "just",
    "make",
    "terraform",
    "poetry",
    "conda",
    "flatpak",
    "snap",
    "zig",
    "dotnet",
    "ip",
    "tmux",
    "magi",
    "melchior",
    "casper",
    "balthasar",
    "bd",
];
/// Programs that run the command after them: `sudo make install`.
const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "time", "nohup", "exec", "xargs", "timeout", "nice", "command",
    "builtin", "watch", "strace", "setsid", "stdbuf",
];
/// The shell's own words.
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "in", "function", "select",
];

/// The command a `shell` call was given; `None` for any other call, or one with no command.
pub(super) fn command_of(name: &str, args: &str) -> Option<String> {
    if name != "shell" {
        return None;
    }
    let serde_json::Value::Object(fields) = serde_json::from_str(args).ok()? else {
        return None;
    };
    fields.get("command")?.as_str().map(ToOwned::to_owned)
}

/// The rows a shell call's command takes in its block: highlighted, one row cut to `body` in a
/// preview, and opened, each of its own lines wrapped in full. `None` when it is not a shell call.
pub(super) fn asked(
    name: &str,
    args: &str,
    detail: Detail,
    style: Style,
    width: u16,
    body: usize,
    lead: usize,
) -> Option<Vec<Line<'static>>> {
    let command = command_of(name, args)?;
    let rows = match detail {
        Detail::Preview => {
            let flat = command.split_whitespace().collect::<Vec<_>>().join(" ");
            let drawn = Line::from(clipped(highlight(&flat, style), body));
            vec![super::frame::inside(drawn, width, style, lead)]
        }
        Detail::Full => command
            .trim()
            .lines()
            .flat_map(|line| {
                let whole = Line::from(highlight(&crate::wrap::expand_tabs(line), style));
                crate::wrap::line(whole, u16::try_from(body).unwrap_or(u16::MAX))
            })
            .map(|part| super::frame::inside(part, width, style, lead))
            .collect(),
    };
    Some(rows)
}

/// Where the reading is: what the next word would be, which is what decides its colour.
struct State {
    /// The next word is a command.
    leading: bool,
    /// The command was a wrapper, so its flags come before the command it runs.
    wrapped: bool,
    /// The command takes a subcommand and has not had one yet.
    owner: bool,
    /// A redirect came, so the next word is a path.
    path_next: bool,
    /// A heredoc came, so the next word is its delimiter.
    delimiter_next: bool,
}

impl State {
    const fn new() -> Self {
        Self {
            leading: true,
            wrapped: false,
            owner: false,
            path_next: false,
            delimiter_next: false,
        }
    }

    /// A value was read: whatever was waiting for one has it.
    fn value(&mut self) {
        self.leading = false;
        self.wrapped = false;
        self.path_next = false;
        self.delimiter_next = false;
    }

    /// What an operator says comes next: a path after a redirect, a delimiter after a heredoc, a
    /// file descriptor after `>&`, the end of a group after `)`, and a new command after the rest.
    fn after(&mut self, op: &str) {
        match op {
            ")" => self.value(),
            "<<" | "<<-" | "<<<" => self.delimiter_next = true,
            _ if op.ends_with('&') && op.starts_with('>') => {}
            _ if op.starts_with('<') || op.starts_with('>') => self.path_next = true,
            _ => *self = Self::new(),
        }
    }

    /// Paint one word by where it sits, splitting `NAME=value` and `--flag=value` at the `=`.
    fn word(&mut self, word: &str, base: Style, out: &mut Vec<Span<'static>>) {
        let paint = |out: &mut Vec<Span<'static>>, text: &str, ink: Color| {
            out.push(Span::styled(text.to_owned(), base.fg(ink)));
        };
        let split = |out: &mut Vec<Span<'static>>, name: &str, value: &str, ink: Color| {
            paint(out, name, ink);
            paint(out, "=", colour::code_operator());
            if !value.is_empty() {
                paint(out, value, value_ink(value));
            }
        };
        if std::mem::take(&mut self.delimiter_next) {
            return paint(out, word, colour::code_string());
        }
        if std::mem::take(&mut self.path_next) {
            return paint(out, word, colour::code_path());
        }
        if self.leading {
            if let Some((name, value)) = assignment(word) {
                // `NAME=value` before a command sets its environment; the command is still to come.
                return split(out, name, value, colour::code_variable());
            }
            if KEYWORDS.contains(&word) {
                self.leading = !matches!(
                    word,
                    "for" | "case" | "select" | "function" | "in" | "fi" | "done" | "esac"
                );
                return paint(out, word, colour::code_keyword());
            }
            if self.wrapped && (word.starts_with('-') || is_number(word)) {
                let ink = if is_number(word) {
                    colour::code_number()
                } else {
                    colour::code_flag()
                };
                return paint(out, word, ink);
            }
            let program = word.rsplit('/').next().unwrap_or(word);
            self.wrapped = WRAPPERS.contains(&program);
            self.leading = self.wrapped;
            self.owner = SUBCOMMANDED.contains(&program);
            return paint(out, word, colour::code_command());
        }
        if word.len() > 1 && word.starts_with('-') {
            return match word.split_once('=') {
                Some((flag, value)) => split(out, flag, value, colour::code_flag()),
                None => paint(out, word, colour::code_flag()),
            };
        }
        if word == "in" {
            return paint(out, word, colour::code_keyword());
        }
        if std::mem::take(&mut self.owner) && is_name(word) {
            return paint(out, word, colour::code_subcommand());
        }
        paint(out, word, value_ink(word));
    }
}

/// One line of shell, as spans on `base`, each piece in the colour of what it is.
#[must_use]
pub fn highlight(line: &str, base: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut state = State::new();
    let mut at = 0;
    while at < chars.len() {
        let from = at;
        let c = chars[at];
        let ink = if c.is_whitespace() {
            while at < chars.len() && chars[at].is_whitespace() {
                at += 1;
            }
            colour::code_argument()
        } else if c == '#' {
            at = chars.len();
            colour::code_comment()
        } else if c == '\'' || c == '"' {
            at = quoted(&chars, at);
            state.value();
            colour::code_string()
        } else if c == '$' {
            let (end, opens) = variable(&chars, at);
            at = end;
            if opens {
                state = State::new();
                colour::code_operator()
            } else {
                state.value();
                colour::code_variable()
            }
        } else if OPERATORS.contains(c) {
            at = operator(&chars, at);
            state.after(&chars[from..at].iter().collect::<String>());
            colour::code_operator()
        } else {
            at = word_end(&chars, at);
            state.word(&chars[from..at].iter().collect::<String>(), base, &mut out);
            continue;
        };
        out.push(Span::styled(
            chars[from..at].iter().collect::<String>(),
            base.fg(ink),
        ));
    }
    out
}

/// Past a quoted string starting at `at`; a double-quoted one skips what a backslash escapes.
fn quoted(chars: &[char], at: usize) -> usize {
    let quote = chars[at];
    let mut end = at + 1;
    while end < chars.len() && chars[end] != quote {
        end += if quote == '"' && chars[end] == '\\' {
            2
        } else {
            1
        };
    }
    (end + 1).min(chars.len())
}

/// Past a `$` expansion at `at`, and whether it opens a command substitution rather than naming a
/// variable: `$(`, `${…}`, `$NAME`, or one of the single-character specials like `$?` and `$1`.
fn variable(chars: &[char], at: usize) -> (usize, bool) {
    let mut end = at + 1;
    match chars.get(end) {
        Some('(') => return (end + 1, true),
        Some('{') => {
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            end += 1;
        }
        Some(c) if "?@#*!$-".contains(*c) || c.is_ascii_digit() => end += 1,
        _ => {
            while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
        }
    }
    (end.min(chars.len()), false)
}

/// Past an operator at `at`: a bracket or backtick alone, or a run like `&&`, `2>&1`'s `>&`, `<<-`.
fn operator(chars: &[char], at: usize) -> usize {
    let mut end = at + 1;
    if matches!(chars[at], '(' | ')' | '`') {
        return end;
    }
    while end < chars.len() && "|&;<>".contains(chars[end]) {
        end += 1;
    }
    if chars[at..end] == ['<', '<'] && chars.get(end) == Some(&'-') {
        end += 1;
    }
    end
}

/// Past a word at `at`, keeping what a backslash escapes inside it.
fn word_end(chars: &[char], at: usize) -> usize {
    let mut end = at;
    while end < chars.len() && !chars[end].is_whitespace() && !STOPS.contains(chars[end]) {
        end += if chars[end] == '\\' { 2 } else { 1 };
    }
    end.min(chars.len())
}

/// `NAME=value`, split at the `=`, when the name is one a shell would take.
fn assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let named = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    named.then_some((name, value))
}

/// The colour of a plain value: a number, a path, or just an argument.
fn value_ink(word: &str) -> Color {
    if is_number(word) {
        colour::code_number()
    } else if is_path(word) {
        colour::code_path()
    } else {
        colour::code_argument()
    }
}

fn is_number(word: &str) -> bool {
    word.chars().any(|c| c.is_ascii_digit())
        && word
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | ':'))
}

/// Something a reader takes for a file: a directory in it, home, a dot in front, a glob, or an
/// extension.
fn is_path(word: &str) -> bool {
    let extended = word.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=5).contains(&ext.len())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    });
    word.contains('/')
        || word.starts_with('~')
        || word.starts_with('.')
        || word.contains(['*', '?'])
        || extended
}

/// A word that can be a subcommand: a name, not a path or a value.
fn is_name(word: &str) -> bool {
    word.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
}

/// `spans` cut to `room` columns, ending in `…` when anything was cut.
pub(super) fn clipped(spans: Vec<Span<'static>>, room: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(|s| crate::wrap::columns(&s.content)).sum();
    if total <= room {
        return spans;
    }
    let keep = room.saturating_sub(1);
    let mut used = 0;
    let mut out = Vec::new();
    for span in spans {
        let wide = crate::wrap::columns(&span.content);
        if used + wide <= keep {
            used += wide;
            out.push(span);
            continue;
        }
        // By columns, not characters: a wide glyph is two, and counting it as one overran the box.
        let mut room = keep - used;
        let kept: String = span
            .content
            .chars()
            .take_while(|c| {
                let wide = unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
                let fits = wide <= room;
                room = room.saturating_sub(wide);
                fits
            })
            .collect();
        out.push(Span::styled(format!("{kept}…"), span.style));
        return out;
    }
    out
}

#[cfg(test)]
#[path = "shell/tests.rs"]
mod tests;
