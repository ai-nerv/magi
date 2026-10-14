use rune::{
    Script, Source,
    ast::{Assignment, Command, Word, WordPiece},
};
use std::collections::BTreeMap;

pub(super) const LIMIT: usize = 262_144;

pub(super) fn name(key: &str) -> bool {
    let mut chars = key.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

pub(super) fn exports(
    text: &str,
    current: &BTreeMap<String, String>,
    strict: bool,
) -> Result<(BTreeMap<String, String>, usize), String> {
    if text.len() > LIMIT || text.contains('\0') {
        return Err("environment input is too large or contains NUL".into());
    }
    let parsed = rune::parse(text);
    if !parsed.is_clean() {
        return Err("invalid shell syntax; no variables changed".into());
    }
    let source = Source::new(text);
    let items: Vec<_> = Script::of(parsed.tree()).items().collect();
    if strict && items.len() != 1 {
        return Err(":env takes assignments, not shell commands".into());
    }
    let mut known = current.clone();
    let mut updates = BTreeMap::new();
    let mut skipped = 0;
    for item in items {
        let command = match item.command() {
            Some(Command::Simple(command)) if !item.is_background() => command,
            _ if strict => return Err(":env takes assignments, not shell commands".into()),
            _ => continue,
        };
        if command.name().and_then(|w| w.literal(&source)).as_deref() != Some("export") {
            continue;
        }
        if command.redirects().next().is_some()
            || command
                .syntax()
                .nodes()
                .next()
                .and_then(Word::cast)
                .is_none()
        {
            if strict {
                return Err("redirections and command prefixes are not supported".into());
            }
            skipped += 1;
            continue;
        }
        let mut batch = BTreeMap::new();
        let mut bytes = updates
            .iter()
            .map(|(key, value): (&String, &String)| key.len() + value.len())
            .sum::<usize>();
        let result = command.syntax().nodes().skip(1).try_for_each(|node| {
            let assignment =
                Assignment::cast(node).ok_or("only KEY=value exports are supported")?;
            let key = assignment.name(&source);
            if !name(key) {
                return Err("invalid environment variable name");
            }
            if assignment.is_appending() || assignment.array().is_some() {
                return Err("only scalar KEY=value exports are supported");
            }
            let value = assignment.value().map_or_else(
                || Ok(String::new()),
                |word| {
                    let value = expand(word, &source, &known)?;
                    let raw = word.text(&source);
                    if (raw == "~" || raw.starts_with("~/")) && value.starts_with('~') {
                        Ok(format!(
                            "{}{}",
                            known.get("HOME").map_or("", String::as_str),
                            value.strip_prefix('~').unwrap_or(&value)
                        ))
                    } else if raw.starts_with('~') && raw != "~" && !raw.starts_with("~/") {
                        Err("~user expansion is not supported")
                    } else {
                        Ok(value)
                    }
                },
            )?;
            if value.len() > LIMIT || value.contains('\0') {
                return Err("environment value is too large or contains NUL");
            }
            bytes = bytes.saturating_sub(
                batch
                    .get(key)
                    .or_else(|| updates.get(key))
                    .map_or(0, |old: &String| key.len() + old.len()),
            ) + key.len()
                + value.len();
            if bytes > LIMIT {
                return Err("environment updates exceed the size limit");
            }
            batch.insert(key.into(), value);
            Ok(())
        });
        if let Err(why) = result {
            if strict {
                return Err(format!("{why}; no variables changed"));
            }
            skipped += 1;
            continue;
        }
        known.extend(batch.clone());
        updates.extend(batch);
    }
    if strict && updates.is_empty() {
        return Err("usage: :env KEY=value [KEY2=value2 ...]".into());
    }
    if updates.values().map(String::len).sum::<usize>() > LIMIT {
        return Err("environment updates exceed the size limit".into());
    }
    Ok((updates, skipped))
}

fn expand(
    word: Word<'_>,
    source: &Source,
    known: &BTreeMap<String, String>,
) -> Result<String, &'static str> {
    let mut out = String::new();
    let mut double = false;
    for piece in word.pieces() {
        match piece {
            WordPiece::Literal(span) => out.push_str(source.slice(span)),
            WordPiece::DoubleQuote(_) => double = !double,
            WordPiece::Quoted(span) => {
                let text = source.slice(span);
                if let Some(inner) = text.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
                    out.push_str(inner);
                } else if let Some(inner) = text.strip_prefix('\\') {
                    if inner == "\n" {
                        continue;
                    }
                    if double && !matches!(inner, "$" | "`" | "\"" | "\\") {
                        out.push('\\');
                    }
                    out.push_str(inner);
                } else {
                    out.push_str(text);
                }
            }
            WordPiece::Parameter(span) => {
                let key = source
                    .slice(span)
                    .strip_prefix('$')
                    .ok_or("unsupported variable expansion")?;
                if !name(key) {
                    return Err("only $KEY and ${KEY} expansions are supported");
                }
                out.push_str(known.get(key).map_or("", String::as_str));
            }
            WordPiece::Expansion(expansion) => {
                let raw = source.slice(expansion.span());
                let key = raw
                    .strip_prefix("${")
                    .and_then(|s| s.strip_suffix('}'))
                    .ok_or("unsupported variable expansion")?;
                if !name(key) {
                    return Err("only $KEY and ${KEY} expansions are supported");
                }
                out.push_str(known.get(key).map_or("", String::as_str));
            }
            WordPiece::Tilde(span) => {
                if source.slice(span) != "~" {
                    return Err("~user expansion is not supported");
                }
                out.push_str(known.get("HOME").map_or("", String::as_str));
            }
            _ => {
                return Err("command, arithmetic, process and ANSI-C expansions are not supported");
            }
        }
        if out.len() > LIMIT {
            return Err("expanded value is too large");
        }
    }
    Ok(out)
}

pub(super) fn path(text: &str, current: &BTreeMap<String, String>) -> Result<String, String> {
    let (mut variables, _) = exports(&format!("export MAGI_SOURCE_PATH={text}"), current, true)?;
    if variables.len() != 1 {
        return Err("usage: :source <file>; quote paths containing spaces".into());
    }
    variables
        .remove("MAGI_SOURCE_PATH")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "usage: :source <file>".into())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
