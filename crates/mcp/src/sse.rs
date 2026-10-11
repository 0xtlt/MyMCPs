//! Server-sent events as the SDK client reads them: a `TextDecoderStream`
//! piped into `eventsource-parser` 3.1.

/// One dispatched event. `id` and `event` do not carry over to the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SseItem {
    Event(SseEvent),
    /// A `retry:` field: how long the server asks to wait before reconnecting.
    Retry(u64),
}

/// Incremental parser. Bytes may be cut anywhere, inside a character or a line.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    /// Bytes of a character cut by the end of a chunk.
    partial_character: Vec<u8>,
    /// Text after the last complete line.
    pending: String,
    started: bool,
    id: Option<String>,
    event: Option<String>,
    data: String,
    data_lines: usize,
}

impl SseParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Vec<SseItem> {
        let mut text = self.decode(chunk);
        if !self.started && !text.is_empty() {
            self.started = true;
            if let Some(stripped) = text.strip_prefix('\u{feff}') {
                text = stripped.to_owned();
            }
        }
        self.pending.push_str(&text);

        let mut items = Vec::new();
        let buffer = std::mem::take(&mut self.pending);
        let mut rest = buffer.as_str();
        while let Some(end) = rest.find(['\r', '\n']) {
            let ends_with_carriage_return = rest.as_bytes()[end] == b'\r';
            // A carriage return at the very end may be the first half of a CRLF.
            if ends_with_carriage_return && end + 1 == rest.len() {
                break;
            }
            let line = &rest[..end];
            let mut next = end + 1;
            if ends_with_carriage_return && rest.as_bytes()[next] == b'\n' {
                next += 1;
            }
            self.line(line, &mut items);
            rest = &rest[next..];
        }
        // An event the stream ends in the middle of is never dispatched.
        self.pending = rest.to_owned();
        items
    }

    /// UTF-8 with invalid sequences replaced, like a non-fatal `TextDecoder`.
    fn decode(&mut self, chunk: &[u8]) -> String {
        let mut bytes = std::mem::take(&mut self.partial_character);
        bytes.extend_from_slice(chunk);
        let mut text = String::with_capacity(bytes.len());
        let mut rest = bytes.as_slice();
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    text.push_str(valid);
                    break;
                }
                Err(error) => {
                    let (valid, invalid) = rest.split_at(error.valid_up_to());
                    text.push_str(&String::from_utf8_lossy(valid));
                    match error.error_len() {
                        Some(length) => {
                            text.push('\u{fffd}');
                            rest = &invalid[length..];
                        }
                        None => {
                            self.partial_character = invalid.to_vec();
                            break;
                        }
                    }
                }
            }
        }
        text
    }

    fn line(&mut self, line: &str, items: &mut Vec<SseItem>) {
        if line.is_empty() {
            self.dispatch(items);
            return;
        }
        // One space after the colon belongs to the syntax, not to the value.
        let value_after = |prefix: &str| {
            line.strip_prefix(prefix)
                .map(|value| value.strip_prefix(' ').unwrap_or(value))
        };
        if let Some(value) = value_after("data:") {
            self.data_line(value);
        } else if let Some(value) = value_after("event:") {
            self.event = Some(value.to_owned()).filter(|value| !value.is_empty());
        } else if let Some(value) = value_after("id:") {
            self.set_id(value);
        } else if line.starts_with(':') {
            // A comment, which servers send to keep the connection open.
        } else {
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line, ""),
            };
            match field {
                "event" => self.event = Some(value.to_owned()).filter(|value| !value.is_empty()),
                "data" => self.data_line(value),
                "id" => self.set_id(value),
                "retry" if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => {
                    items.push(SseItem::Retry(value.parse().unwrap_or(u64::MAX)));
                }
                // An unknown field or an invalid `retry` is an error nobody listens to.
                _ => {}
            }
        }
    }

    fn data_line(&mut self, value: &str) {
        if self.data_lines > 0 {
            self.data.push('\n');
        }
        self.data.push_str(value);
        self.data_lines += 1;
    }

    fn set_id(&mut self, value: &str) {
        if !value.contains('\0') {
            self.id = Some(value.to_owned());
        }
    }

    fn dispatch(&mut self, items: &mut Vec<SseItem>) {
        let id = self.id.take();
        let event = self.event.take();
        let data = std::mem::take(&mut self.data);
        if std::mem::take(&mut self.data_lines) > 0 {
            items.push(SseItem::Event(SseEvent { id, event, data }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(chunks: &[&[u8]]) -> Vec<SseItem> {
        let mut parser = SseParser::new();
        chunks.iter().flat_map(|chunk| parser.feed(chunk)).collect()
    }

    fn event(id: Option<&str>, event: Option<&str>, data: &str) -> SseItem {
        SseItem::Event(SseEvent {
            id: id.map(str::to_owned),
            event: event.map(str::to_owned),
            data: data.to_owned(),
        })
    }

    /// `tests/fixtures/sse.json`: what `TextDecoderStream` piped into
    /// `eventsource-parser` 3.1.1 makes of byte streams, each cut in many ways.
    #[test]
    fn reads_streams_exactly_as_the_parser_of_the_sdk_does() {
        #[derive(serde::Deserialize)]
        struct Fixtures {
            cases: Vec<Case>,
        }
        #[derive(serde::Deserialize)]
        struct Case {
            input: String,
            chunkings: Vec<Vec<String>>,
            items: Vec<serde_json::Value>,
        }
        fn bytes(hex: &str) -> Vec<u8> {
            (0..hex.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
                .collect()
        }

        let fixtures: Fixtures =
            serde_json::from_str(include_str!("../tests/fixtures/sse.json")).unwrap();
        assert!(fixtures.cases.len() > 20);
        for case in &fixtures.cases {
            let expected: Vec<SseItem> = case
                .items
                .iter()
                .map(|item| match item.get("retry") {
                    Some(retry) => SseItem::Retry(retry.as_f64().unwrap() as u64),
                    None => event(
                        item["id"].as_str(),
                        item["event"].as_str(),
                        item["data"].as_str().unwrap(),
                    ),
                })
                .collect();
            for chunking in &case.chunkings {
                let chunks: Vec<Vec<u8>> = chunking.iter().map(|chunk| bytes(chunk)).collect();
                let mut parser = SseParser::new();
                let got: Vec<SseItem> =
                    chunks.iter().flat_map(|chunk| parser.feed(chunk)).collect();
                assert_eq!(
                    got,
                    expected,
                    "input {:?} cut as {:?}",
                    String::from_utf8_lossy(&bytes(&case.input)),
                    chunks
                        .iter()
                        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                        .collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn reads_the_frames_the_sdk_server_writes() {
        assert_eq!(
            events(&[
                b"event: message\ndata: {\"a\":1}\n\n",
                b"event: message\nid: 7\ndata: {}\n\n"
            ]),
            vec![
                event(None, Some("message"), "{\"a\":1}"),
                event(Some("7"), Some("message"), "{}")
            ]
        );
    }

    #[test]
    fn reads_an_event_cut_anywhere() {
        let whole = "event: message\ndata: {\"text\":\"h\u{e9}llo \u{1f600}\"}\n\n".as_bytes();
        for cut in 1..whole.len() {
            assert_eq!(
                events(&[&whole[..cut], &whole[cut..]]),
                vec![event(
                    None,
                    Some("message"),
                    "{\"text\":\"h\u{e9}llo \u{1f600}\"}"
                )],
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn accepts_every_line_ending() {
        let expected = vec![event(None, None, "a\nb"), event(None, Some("x"), "c")];
        assert_eq!(
            events(&[b"data: a\r\ndata: b\r\n\r\nevent: x\r\ndata: c\r\n\r\n"]),
            expected
        );
        assert_eq!(
            events(&[b"data: a\rdata: b\r\revent: x\rdata: c\r\r", b"\n"]),
            expected
        );
        // A CRLF cut between its two bytes is still one line ending.
        assert_eq!(
            events(&[
                b"data: a\r",
                b"\ndata: b\r",
                b"\n\r",
                b"\nevent: x\ndata: c\n\n"
            ]),
            expected
        );
    }

    #[test]
    fn drops_what_the_stream_did_not_finish() {
        assert_eq!(events(&[b"data: {\"a\":1}\n"]), vec![]);
        assert_eq!(
            events(&[b"data: {\"a\":1}\n\ndata: half"]),
            vec![event(None, None, "{\"a\":1}")]
        );
    }

    #[test]
    fn ignores_comments_and_unknown_fields_and_reports_retry() {
        assert_eq!(
            events(&[b": keepalive\n\nretry: 3000\nretry: soon\nfoo: bar\ndata\ndata:x\n\n"]),
            vec![SseItem::Retry(3000), event(None, None, "\nx")]
        );
    }

    #[test]
    fn dispatches_an_empty_data_line_and_forgets_the_id_after_each_event() {
        assert_eq!(
            events(&[b"id: 1\ndata:\n\ndata: next\n\nid: a\0b\ndata: z\n\nid: only\n\n"]),
            vec![
                event(Some("1"), None, ""),
                event(None, None, "next"),
                event(None, None, "z")
            ]
        );
    }

    #[test]
    fn ignores_a_byte_order_mark_and_replaces_invalid_bytes() {
        assert_eq!(
            events(&[b"\xef\xbb\xbfdata: a\n\n"]),
            vec![event(None, None, "a")]
        );
        assert_eq!(
            events(&[b"data: a\xffb\n\n"]),
            vec![event(None, None, "a\u{fffd}b")]
        );
    }
}
