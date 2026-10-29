use super::{Failure, valid_name};

#[derive(Debug, PartialEq, Eq)]
pub struct Invocation<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
}

pub fn invocation(text: &str) -> Result<Option<Invocation<'_>>, Failure> {
    let Some(rest) = text.strip_prefix("/skill") else {
        return Ok(None);
    };
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return Ok(None);
    }
    let rest = rest.trim_start_matches(char::is_whitespace);
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = rest.get(..end).ok_or(Failure::Invocation)?;
    if !valid_name(name) {
        return Err(Failure::Invocation);
    }
    let tail = rest.get(end..).ok_or(Failure::Invocation)?;
    let arguments = tail
        .chars()
        .next()
        .and_then(|separator| tail.get(separator.len_utf8()..))
        .unwrap_or_default();
    Ok(Some(Invocation { name, arguments }))
}
