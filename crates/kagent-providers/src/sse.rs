//! Minimal SSE (server-sent events) parser for streaming model APIs.
//!
//! Handles `event:`/`data:` lines, comments, `\r\n`, and events split across
//! arbitrary chunk boundaries.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes; returns completed events.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut out = Vec::new();
        // Only process up to the last newline; the rest stays buffered.
        while let Some(pos) = self.buffer.find('\n') {
            let mut line = self.buffer[..pos].to_string();
            self.buffer.drain(..=pos);
            if line.ends_with('\r') {
                line.pop();
            }
            if line.is_empty() {
                if let Some(ev) = self.take_event() {
                    out.push(ev);
                }
                continue;
            }
            if line.starts_with(':') {
                continue; // comment / keepalive
            }
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line.as_str(), ""),
            };
            match field {
                "event" => self.event = Some(value.to_string()),
                "data" => self.data.push(value.to_string()),
                _ => {}
            }
        }
        out
    }

    /// Flush a trailing event that was not terminated by a blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.buffer.is_empty() {
            let rest = std::mem::take(&mut self.buffer);
            let mut more = self.feed(rest.as_bytes());
            // Any final line without a newline still needs dispatching.
            if !self.buffer.is_empty() {
                let line = std::mem::take(&mut self.buffer);
                self.handle_line(&line);
            }
            out.append(&mut more);
        }
        if let Some(ev) = self.take_event() {
            out.push(ev);
        }
        out
    }

    fn handle_line(&mut self, line: &str) {
        let mut line = line.to_string();
        if line.ends_with('\r') {
            line.pop();
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line.as_str(), ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            _ => {}
        }
    }

    fn take_event(&mut self) -> Option<SseEvent> {
        let data = std::mem::take(&mut self.data);
        let event = self.event.take();
        if event.is_none() && data.is_empty() {
            return None;
        }
        Some(SseEvent { event, data: data.join("\n") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_stream() {
        let mut p = SseParser::new();
        let evs = p.feed(b"event: message\ndata: {\"a\":1}\n\ndata: {\"b\":2}\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event.as_deref(), Some("message"));
        assert_eq!(evs[0].data, "{\"a\":1}");
        assert_eq!(evs[1].event, None);
        assert_eq!(evs[1].data, "{\"b\":2}");
    }

    #[test]
    fn handles_crlf_and_comments() {
        let mut p = SseParser::new();
        let evs = p.feed(b": keepalive\r\n\r\nevent: x\r\ndata: hi\r\n\r\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("x"));
        assert_eq!(evs[0].data, "hi");
    }

    #[test]
    fn splits_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.feed(b"event: message\nda").is_empty());
        assert!(p.feed(b"ta: {\"x\":").is_empty());
        assert!(p.feed(b"1}\n").is_empty());
        let evs = p.feed(b"\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "{\"x\":1}");
    }

    #[test]
    fn multiple_data_lines_join_with_newline() {
        let mut p = SseParser::new();
        let evs = p.feed(b"data: line1\ndata: line2\n\n");
        assert_eq!(evs[0].data, "line1\nline2");
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut p = SseParser::new();
        assert!(p.feed(b"data: tail").is_empty());
        let evs = p.finish();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "tail");
    }
}
