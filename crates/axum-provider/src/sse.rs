//! One SSE parser, for every provider.
//!
//! Pi ended up with four because four vendors frame slightly differently. There is only one
//! framing in the spec — `field: value` lines, blank line ends an event, `\r`, `\n`, or `\r\n`
//! all terminate — so there is one parser here and vendors differ in payload, not in framing.

/// One server-sent event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Event {
    /// The `event:` field, empty when absent.
    pub name: String,
    /// The `data:` field, with multiple data lines joined by newlines.
    pub data: String,
}

/// Accumulates bytes and yields whole events.
#[derive(Debug, Default)]
pub struct Parser {
    buffer: String,
    name: String,
    data: Vec<String>,
}

impl Parser {
    /// A parser with nothing buffered.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk and take whatever events it completed.
    ///
    /// A chunk may split a line, a line may split a field, and an event may span chunks; only
    /// a blank line completes one, so a partial tail stays buffered rather than being guessed
    /// at.
    pub fn push(&mut self, chunk: &str) -> Vec<Event> {
        self.buffer.push_str(chunk);
        let mut out = Vec::new();

        while let Some(end) = self.buffer.find(['\n', '\r']) {
            let line: String = self.buffer[..end].to_owned();
            // A `\r\n` is one terminator, not two, and dropping only the `\r` would leave a
            // blank line that falsely ends the event.
            let skip = if self.buffer[end..].starts_with("\r\n") {
                2
            } else {
                1
            };
            self.buffer.drain(..end + skip);

            if line.is_empty() {
                if let Some(event) = self.take() {
                    out.push(event);
                }
                continue;
            }
            self.field(&line);
        }
        out
    }

    /// Take whatever is buffered, for a stream that ended without a final blank line.
    pub fn finish(&mut self) -> Option<Event> {
        let trailing = std::mem::take(&mut self.buffer);
        if !trailing.is_empty() {
            self.field(&trailing);
        }
        self.take()
    }

    fn field(&mut self, line: &str) {
        // A comment keeps the connection warm and carries nothing.
        if line.starts_with(':') {
            return;
        }
        let (name, value) = match line.split_once(':') {
            Some((name, value)) => (name, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match name {
            "event" => self.name = value.to_owned(),
            "data" => self.data.push(value.to_owned()),
            _ => {}
        }
    }

    fn take(&mut self) -> Option<Event> {
        if self.data.is_empty() && self.name.is_empty() {
            return None;
        }
        Some(Event {
            name: std::mem::take(&mut self.name),
            data: std::mem::take(&mut self.data).join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(chunks: &[&str]) -> Vec<Event> {
        let mut parser = Parser::new();
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(parser.push(chunk));
        }
        out.extend(parser.finish());
        out
    }

    #[test]
    fn a_whole_event_parses() {
        assert_eq!(
            parse(&["event: delta\ndata: hello\n\n"]),
            vec![Event {
                name: "delta".into(),
                data: "hello".into()
            }]
        );
    }

    #[test]
    fn an_event_split_across_chunks_still_parses() {
        assert_eq!(
            parse(&["event: del", "ta\ndata: hel", "lo\n\n"]),
            vec![Event {
                name: "delta".into(),
                data: "hello".into()
            }]
        );
    }

    #[test]
    fn all_three_line_terminators_work() {
        for terminator in ["\n", "\r", "\r\n"] {
            let source = format!("data: x{terminator}{terminator}");
            assert_eq!(parse(&[&source]).len(), 1, "terminator {terminator:?}");
        }
    }

    #[test]
    fn crlf_is_one_terminator_not_a_blank_line() {
        let events = parse(&["event: a\r\ndata: one\r\n\r\ndata: two\r\n\r\n"]);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].data, "one");
    }

    #[test]
    fn multiple_data_lines_join_with_newlines() {
        assert_eq!(parse(&["data: a\ndata: b\n\n"])[0].data, "a\nb");
    }

    #[test]
    fn comments_are_ignored() {
        let events = parse(&[": keep-alive\ndata: real\n\n"]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "real");
    }

    #[test]
    fn a_field_with_no_space_after_the_colon_still_parses() {
        assert_eq!(parse(&["data:tight\n\n"])[0].data, "tight");
    }

    #[test]
    fn a_stream_that_ends_without_a_blank_line_still_yields_its_event() {
        assert_eq!(parse(&["data: last\n"])[0].data, "last");
    }

    #[test]
    fn an_empty_stream_yields_nothing() {
        assert!(parse(&[""]).is_empty());
    }

    #[test]
    fn back_to_back_events_stay_separate() {
        let events = parse(&["data: one\n\ndata: two\n\ndata: three\n\n"]);
        assert_eq!(events.len(), 3);
        assert_eq!(events[2].data, "three");
    }
}
