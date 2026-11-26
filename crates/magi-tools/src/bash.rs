//! Bash command analysis for permissions: parses commands with casper's `tree-sitter-bash` driver
//! when installed, falling back to a POSIX shell lexer so every real invoked program is checked
//! while quoted text never false-triggers deny rules.

use magi_proto::permit::{Grant, Scope};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::time::{Duration, Instant};

const CSTR_VERSION: u32 = 1;
const RECORD: usize = 36;
const HAS_ERROR: u16 = 1 << 2;
const MOST_NODES: usize = 1 << 18;
const PARSE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParserKind {
    TreeSitter,
    Lexical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub program: String,
    pub has_assignments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub commands: Vec<ParsedCommand>,
    pub ambiguous: bool,
    pub parser: ParserKind,
}

/// Analyze a bash command using `tree-sitter-bash` when its driver is installed, else POSIX lexer.
#[must_use]
pub fn analyze(command: &str) -> Analysis {
    analyze_with_driver(command, find_bash_driver().as_deref(), 0)
}

/// Extract the first invoked program name for display / permission prompt subject.
#[must_use]
pub fn first_program(command: &str) -> String {
    analyze(command)
        .commands
        .into_iter()
        .next()
        .map(|cmd| cmd.program)
        .unwrap_or_default()
}

/// Whether `grants` cover every invoked command in `command` (fail-closed on ambiguity or env overrides).
#[must_use]
pub fn covers_run(grants: &[Grant], command: &str) -> bool {
    if grants
        .iter()
        .any(|g| g.verb == "run" && matches!(g.scope, Scope::Anything))
    {
        return true;
    }
    let analysis = analyze(command);
    if analysis.ambiguous || analysis.commands.is_empty() {
        return false;
    }
    if analysis.commands.iter().any(|cmd| cmd.has_assignments) {
        return false;
    }
    analysis.commands.iter().all(|cmd| {
        let base = basename(&cmd.program);
        grants.iter().any(|g| {
            g.verb == "run"
                && matches!(
                    &g.scope,
                    Scope::Program { program } if program == &cmd.program || program == base
                )
        })
    })
}

/// Whether a deny/ask `rule` names any real command invoked by `command`.
#[must_use]
pub fn names_run(rule: &Grant, command: &str) -> bool {
    if rule.verb != "run" {
        return false;
    }
    match &rule.scope {
        Scope::Anything => true,
        Scope::Program { program } => {
            let analysis = analyze(command);
            analysis.commands.iter().any(|cmd| {
                let base = basename(&cmd.program);
                base == program || cmd.program == *program
            })
        }
        _ => false,
    }
}

/// Whether `command` contains dynamic/unparseable command invocations that require asking the user.
#[must_use]
pub fn is_ambiguous(command: &str) -> bool {
    analyze(command).ambiguous
}

fn basename(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

fn find_bash_driver() -> Option<PathBuf> {
    if let Some(paths) = std::env::var_os("CASPER_GRAMMARS") {
        for dir in std::env::split_paths(&paths) {
            for candidate in [dir.join("bin").join("bash"), dir.join("bash")] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })?;
    let path = base
        .join("casper")
        .join("grammars")
        .join("bin")
        .join("bash");
    path.is_file().then_some(path)
}

fn analyze_with_driver(command: &str, driver: Option<&Path>, depth: usize) -> Analysis {
    if depth >= MAX_DEPTH {
        return Analysis {
            commands: Vec::new(),
            ambiguous: true,
            parser: ParserKind::Lexical,
        };
    }
    if let Some(driver) = driver
        && let Some(tree) = run_driver(driver, command.as_bytes())
    {
        return walk_tree(&tree, command.as_bytes(), Some(driver), depth);
    }
    lexical_analyze(command, driver, depth)
}

#[derive(Default)]
struct TsNode {
    kind: u16,
    field: u16,
    flags: u16,
    start_byte: u32,
    end_byte: u32,
    kids: u32,
    count: u32,
}

#[derive(Default)]
struct TsTree {
    kinds: Vec<String>,
    fields: Vec<String>,
    nodes: Vec<TsNode>,
    kids: Vec<u32>,
}

impl TsTree {
    fn node(&self, at: u32) -> Option<&TsNode> {
        self.nodes.get(at as usize)
    }

    fn kind(&self, node: &TsNode) -> &str {
        self.kinds
            .get(usize::from(node.kind))
            .map_or("", String::as_str)
    }

    fn field_name(&self, node: &TsNode) -> &str {
        node.field
            .checked_sub(1)
            .and_then(|idx| self.fields.get(usize::from(idx)))
            .map_or("", String::as_str)
    }

    fn children(&self, node: &TsNode) -> &[u32] {
        let from = node.kids as usize;
        self.kids
            .get(from..from + node.count as usize)
            .unwrap_or_default()
    }
}

fn run_driver(driver: &Path, source: &[u8]) -> Option<TsTree> {
    let mut child = ProcessCommand::new(driver)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let bytes = source.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
    }
    let out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let res = out.take(4 * 1024 * 1024).read_to_end(&mut buf);
        let _ = tx.send(res.map(|_| buf));
    });
    let deadline = Instant::now() + PARSE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let bytes = rx.recv_timeout(Duration::from_millis(200)).ok()?.ok()?;
    decode_cstr(&bytes, source.len())
}

fn decode_cstr(bytes: &[u8], length: usize) -> Option<TsTree> {
    let mut at = 0usize;
    let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
        let slice = bytes.get(*at..*at + n)?;
        *at += n;
        Some(slice)
    };
    let read_u32 = |at: &mut usize| -> Option<u32> {
        let raw: [u8; 4] = take(at, 4)?.try_into().ok()?;
        Some(u32::from_le_bytes(raw))
    };
    let read_u16 = |at: &mut usize| -> Option<u16> {
        let raw: [u8; 2] = take(at, 2)?.try_into().ok()?;
        Some(u16::from_le_bytes(raw))
    };
    let read_name = |at: &mut usize| -> Option<String> {
        let len = usize::from(read_u16(at)?);
        let raw = take(at, len)?;
        std::str::from_utf8(raw).ok().map(str::to_owned)
    };
    if take(&mut at, 4)? != b"CSTR" || read_u32(&mut at)? != CSTR_VERSION {
        return None;
    }
    let _rev = read_name(&mut at)?;
    let symbols = read_u32(&mut at)? as usize;
    let mut kinds = Vec::with_capacity(symbols + 1);
    for _ in 0..=symbols {
        kinds.push(read_name(&mut at)?);
    }
    let field_count = read_u32(&mut at)? as usize;
    let mut fields = Vec::with_capacity(field_count);
    for _ in 0..field_count {
        fields.push(read_name(&mut at)?);
    }
    let count = read_u32(&mut at)? as usize;
    if count == 0 || count > MOST_NODES || count * RECORD != bytes.len().saturating_sub(at) {
        return None;
    }
    let mut tree = TsTree {
        kinds,
        fields,
        nodes: Vec::with_capacity(count),
        kids: Vec::with_capacity(count.saturating_sub(1)),
    };
    let mut open: Vec<(u32, u32)> = Vec::new();
    for idx in 0..count {
        let kind = read_u16(&mut at)?;
        let field = read_u16(&mut at)?;
        let flags = read_u16(&mut at)?;
        let _pad = read_u16(&mut at)?;
        let start_byte = read_u32(&mut at)?;
        let end_byte = read_u32(&mut at)?;
        let _sr = read_u32(&mut at)?;
        let _sc = read_u32(&mut at)?;
        let _er = read_u32(&mut at)?;
        let _ec = read_u32(&mut at)?;
        let kid_count = read_u32(&mut at)?;
        if usize::from(kind) > symbols
            || usize::from(field) > field_count
            || start_byte > end_byte
            || end_byte as usize > length
        {
            return None;
        }
        let idx_u32 = u32::try_from(idx).ok()?;
        if let Some((parent, filled)) = open.last_mut() {
            let slot = tree.nodes.get(*parent as usize)?.kids + *filled;
            *tree.kids.get_mut(slot as usize)? = idx_u32;
            *filled += 1;
        }
        let kids_start = u32::try_from(tree.kids.len()).ok()?;
        tree.kids.resize(tree.kids.len() + kid_count as usize, 0);
        tree.nodes.push(TsNode {
            kind,
            field,
            flags,
            start_byte,
            end_byte,
            kids: kids_start,
            count: kid_count,
        });
        if kid_count > 0 {
            open.push((idx_u32, 0));
        }
        while let Some((parent, filled)) = open.last() {
            if *filled == tree.nodes.get(*parent as usize)?.count {
                open.pop();
            } else {
                break;
            }
        }
    }
    open.is_empty().then_some(tree)
}

fn walk_tree(tree: &TsTree, source: &[u8], driver: Option<&Path>, depth: usize) -> Analysis {
    let mut commands = Vec::new();
    let mut ambiguous = false;
    if let Some(root) = tree.node(0) {
        if root.flags & HAS_ERROR != 0 {
            ambiguous = true;
        }
        visit_node(
            tree,
            source,
            0,
            driver,
            depth,
            &mut commands,
            &mut ambiguous,
        );
    }
    Analysis {
        commands,
        ambiguous,
        parser: ParserKind::TreeSitter,
    }
}

fn visit_node(
    tree: &TsTree,
    source: &[u8],
    at: u32,
    driver: Option<&Path>,
    depth: usize,
    commands: &mut Vec<ParsedCommand>,
    ambiguous: &mut bool,
) {
    let Some(node) = tree.node(at) else {
        return;
    };
    let kind = tree.kind(node);
    if kind == "ERROR" || kind == "MISSING" {
        *ambiguous = true;
    }
    if kind == "command" {
        extract_ts_command(tree, source, node, driver, depth, commands, ambiguous);
    }
    for &kid in tree.children(node) {
        visit_node(tree, source, kid, driver, depth, commands, ambiguous);
    }
}

fn extract_ts_command(
    tree: &TsTree,
    source: &[u8],
    node: &TsNode,
    driver: Option<&Path>,
    depth: usize,
    commands: &mut Vec<ParsedCommand>,
    ambiguous: &mut bool,
) {
    let mut has_assignments = false;
    let mut name_node: Option<&TsNode> = None;
    let mut args: Vec<&TsNode> = Vec::new();

    for &kid_id in tree.children(node) {
        let Some(kid) = tree.node(kid_id) else {
            continue;
        };
        let kid_kind = tree.kind(kid);
        let field = tree.field_name(kid);
        if kid_kind == "variable_assignment" {
            has_assignments = true;
        } else if field == "name" || kid_kind == "command_name" {
            name_node = Some(kid);
        } else if field == "argument" {
            args.push(kid);
        }
    }

    let Some(name_node) = name_node else {
        return;
    };
    if contains_dynamic(tree, name_node) {
        *ambiguous = true;
        return;
    }
    let raw = node_text(source, name_node);
    let program = unquote_token(raw);
    if program.is_empty() || has_glob(&program) {
        *ambiguous = true;
        return;
    }
    let base = basename(&program);
    if matches!(base, "eval" | "exec" | "source" | ".") {
        *ambiguous = true;
    }
    commands.push(ParsedCommand {
        program: program.clone(),
        has_assignments,
    });

    if matches!(base, "sh" | "bash" | "dash" | "zsh") {
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let text = unquote_token(node_text(source, arg));
            if text == "-c" || text == "-lc" || text == "-ec" {
                if let Some(script_node) = iter.next() {
                    if contains_dynamic(tree, script_node) {
                        *ambiguous = true;
                    } else {
                        let inner = unquote_token(node_text(source, script_node));
                        let sub = analyze_with_driver(&inner, driver, depth + 1);
                        commands.extend(sub.commands);
                        *ambiguous |= sub.ambiguous;
                    }
                }
                break;
            }
        }
    }
}

fn contains_dynamic(tree: &TsTree, node: &TsNode) -> bool {
    let kind = tree.kind(node);
    if matches!(
        kind,
        "expansion"
            | "simple_expansion"
            | "command_substitution"
            | "process_substitution"
            | "arithmetic_expansion"
            | "brace_expression"
            | "ERROR"
    ) {
        return true;
    }
    tree.children(node)
        .iter()
        .filter_map(|&id| tree.node(id))
        .any(|kid| contains_dynamic(tree, kid))
}

fn node_text<'a>(source: &'a [u8], node: &TsNode) -> &'a str {
    source
        .get(node.start_byte as usize..node.end_byte as usize)
        .and_then(|s| std::str::from_utf8(s).ok())
        .unwrap_or("")
}

fn unquote_token(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            (None, '\'' | '"') => quote = Some(c),
            (_, ch) => out.push(ch),
        }
    }
    out
}

fn has_glob(token: &str) -> bool {
    token.contains(['*', '?', '['])
}

/// Pure-Rust POSIX shell fallback when no `tree-sitter-bash` driver binary is installed.
fn lexical_analyze(command: &str, driver: Option<&Path>, depth: usize) -> Analysis {
    let mut commands = Vec::new();
    let mut ambiguous = false;
    let mut at_start = true;
    let mut has_assignments = false;
    let mut skip_redirect_target = false;
    let mut current_shell: Option<bool> = None;

    let mut chars = command.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c == ' ' || c == '\t' || c == '\r' {
            chars.next();
            continue;
        }
        if c == '#' && at_start {
            for ch in chars.by_ref() {
                if ch == '\n' {
                    break;
                }
            }
            continue;
        }
        if matches!(c, '\n' | ';' | '&' | '|' | '(' | ')') {
            chars.next();
            if matches!(c, '&' | '|') && chars.peek() == Some(&c) {
                chars.next();
            }
            at_start = true;
            has_assignments = false;
            skip_redirect_target = false;
            current_shell = None;
            continue;
        }
        if matches!(c, '<' | '>') {
            chars.next();
            if chars.peek() == Some(&c) || chars.peek() == Some(&'&') {
                chars.next();
            }
            skip_redirect_target = true;
            continue;
        }

        let (token, quoted, dynamic, unclosed, subs) = read_word(&mut chars);
        if unclosed {
            ambiguous = true;
        }
        for sub in subs {
            let nested = analyze_with_driver(&sub, driver, depth + 1);
            commands.extend(nested.commands);
            ambiguous |= nested.ambiguous;
        }
        if skip_redirect_target {
            skip_redirect_target = false;
            continue;
        }
        if at_start {
            if !quoted && is_assignment(&token) {
                has_assignments = true;
                continue;
            }
            if matches!(
                token.as_str(),
                "if" | "then"
                    | "else"
                    | "elif"
                    | "fi"
                    | "do"
                    | "done"
                    | "for"
                    | "while"
                    | "until"
                    | "case"
                    | "esac"
                    | "!"
                    | "{"
                    | "}"
            ) && !quoted
            {
                continue;
            }
            if dynamic || token.is_empty() || has_glob(&token) {
                ambiguous = true;
            } else {
                let base = basename(&token);
                if matches!(base, "eval" | "exec" | "source" | ".") {
                    ambiguous = true;
                }
                commands.push(ParsedCommand {
                    program: token.clone(),
                    has_assignments,
                });
                if matches!(base, "sh" | "bash" | "dash" | "zsh") {
                    current_shell = Some(false);
                }
            }
            at_start = false;
        } else if let Some(expect_script) = current_shell {
            if expect_script {
                if dynamic {
                    ambiguous = true;
                } else {
                    let nested = analyze_with_driver(&token, driver, depth + 1);
                    commands.extend(nested.commands);
                    ambiguous |= nested.ambiguous;
                }
                current_shell = None;
            } else if matches!(token.as_str(), "-c" | "-lc" | "-ec") {
                current_shell = Some(true);
            }
        }
    }

    Analysis {
        commands,
        ambiguous,
        parser: ParserKind::Lexical,
    }
}

fn read_word(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> (String, bool, bool, bool, Vec<String>) {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut saw_quote = false;
    let mut dynamic = false;
    let mut subs = Vec::new();

    while let Some(&c) = chars.peek() {
        match (quote, c) {
            (None, ' ' | '\t' | '\r' | '\n' | ';' | '&' | '|' | '(' | ')' | '<' | '>') => break,
            (Some('\''), '\'') | (Some('"'), '"') => {
                chars.next();
                quote = None;
            }
            (Some('"'), '\\') | (None, '\\') => {
                chars.next();
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            (None, '\'' | '"') => {
                chars.next();
                quote = Some(c);
                saw_quote = true;
            }
            (Some('\''), ch) => {
                chars.next();
                out.push(ch);
            }
            (_, '`') => {
                chars.next();
                let mut inner = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '`' {
                        closed = true;
                        break;
                    }
                    inner.push(ch);
                }
                if !closed {
                    return (out, saw_quote, true, true, subs);
                }
                subs.push(inner);
                dynamic = true;
            }
            (_, '$') => {
                chars.next();
                if chars.peek() == Some(&'(') {
                    chars.next();
                    let mut inner = String::new();
                    let mut depth = 1usize;
                    for ch in chars.by_ref() {
                        if ch == '(' {
                            depth += 1;
                            inner.push(ch);
                        } else if ch == ')' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            inner.push(ch);
                        } else {
                            inner.push(ch);
                        }
                    }
                    if depth != 0 {
                        return (out, saw_quote, true, true, subs);
                    }
                    subs.push(inner);
                }
                dynamic = true;
            }
            (_, ch) => {
                chars.next();
                out.push(ch);
            }
        }
    }
    (out, saw_quote, dynamic, quote.is_some(), subs)
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog(name: &str) -> Grant {
        Grant {
            verb: "run".to_owned(),
            scope: Scope::Program {
                program: name.to_owned(),
            },
        }
    }

    #[test]
    fn parses_compound_commands_and_ignores_quoted_arguments() {
        let compound = analyze("git diff && rm -rf /tmp/x");
        let names: Vec<&str> = compound
            .commands
            .iter()
            .map(|c| c.program.as_str())
            .collect();
        assert_eq!(names, vec!["git", "rm"]);
        assert!(!compound.ambiguous);

        let quoted = analyze(r#"git commit -m "remove rm && sudo from scripts""#);
        let qnames: Vec<&str> = quoted.commands.iter().map(|c| c.program.as_str()).collect();
        assert_eq!(qnames, vec!["git"]);
        assert!(!names_run(
            &prog("rm"),
            r#"git commit -m "remove rm && sudo""#
        ));
        assert!(!names_run(
            &prog("sudo"),
            r#"git commit -m "remove rm && sudo""#
        ));
        assert!(names_run(&prog("rm"), "git diff && rm -rf /"));
    }

    #[test]
    fn covers_pipeline_only_when_every_command_is_granted() {
        let git_only = vec![prog("git")];
        let git_and_head = vec![prog("git"), prog("head")];
        assert!(!covers_run(&git_only, "git log | head -n 10"));
        assert!(covers_run(&git_and_head, "git log | head -n 10"));
        assert!(!covers_run(&git_and_head, "git diff && rm -rf /"));
        assert!(!covers_run(&git_only, "PATH=/evil git status"));
        assert!(!covers_run(&git_only, "eval 'git status'"));
    }

    #[test]
    fn catches_subshells_command_substitutions_and_sh_c() {
        assert!(names_run(&prog("rm"), "echo $(rm -rf /tmp/a)"));
        assert!(names_run(&prog("rm"), "sh -c 'true && /bin/rm -f x'"));
        assert!(is_ambiguous("$CMD --flag"));
        assert!(is_ambiguous("eval 'echo hi'"));
        assert!(!is_ambiguous("git log --format='%s [ok]'"));
    }

    #[test]
    fn lexical_fallback_matches_treesitter_on_core_cases() {
        for (cmd, expected_progs, expected_ambig) in [
            ("git status", vec!["git"], false),
            ("git log | grep foo", vec!["git", "grep"], false),
            (r#"echo "rm -rf /""#, vec!["echo"], false),
            ("sh -c 'rm x'", vec!["sh", "rm"], false),
            ("$DYN arg", vec![], true),
        ] {
            let lex = lexical_analyze(cmd, None, 0);
            let got: Vec<&str> = lex.commands.iter().map(|c| c.program.as_str()).collect();
            assert_eq!(got, expected_progs, "lexical {cmd}");
            assert_eq!(lex.ambiguous, expected_ambig, "lexical ambig {cmd}");
            if let Some(driver) = find_bash_driver() {
                let ts = analyze_with_driver(cmd, Some(&driver), 0);
                assert_eq!(ts.parser, ParserKind::TreeSitter);
                let ts_got: Vec<&str> = ts.commands.iter().map(|c| c.program.as_str()).collect();
                assert_eq!(ts_got, expected_progs, "ts {cmd}");
                assert_eq!(ts.ambiguous, expected_ambig, "ts ambig {cmd}");
            }
        }
    }
}
