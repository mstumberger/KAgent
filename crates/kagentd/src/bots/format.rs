//! Text for Telegram (bots.md section 3): an answer split into messages, Markdown as
//! Telegram HTML, and the lines that are not text. Pure functions; the tests are in
//! `tests.rs`.

/// Telegram takes 1-4096 characters in a message, counted after the entities are parsed.
pub const MAX_MESSAGE: usize = 4096;
/// A message of the chat is cut here, in characters, before it becomes a turn's input.
pub const MAX_INPUT: usize = 4000;

/// `text` cut to `max` characters, never inside one.
pub fn cut(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// The text without its directive lines: `::name{...}`, which may run over more than
/// one line until its closing brace. One quoted in a code block stays.
pub fn drop_directives(text: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let (mut code, mut inside) = (false, false);
    for line in text.lines() {
        let start = line.trim_start();
        if inside {
            // The directive runs to its closing brace; what follows it on that line is text.
            if let Some(i) = start.find('}') {
                inside = false;
                keep(&mut kept, &start[i + 1..]);
            }
            continue;
        }
        if start.starts_with("```") || start.starts_with("~~~") {
            code = !code;
        } else if !code {
            if let Some(body) = directive_body(start) {
                match body.find('}') {
                    Some(i) => keep(&mut kept, &body[i + 1..]),
                    None => inside = true,
                }
                continue;
            }
        }
        kept.push(line);
    }
    kept.join("\n")
}

/// What follows the `{` of a line that is a directive (`::name{`), else `None`.
fn directive_body(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("::")?;
    let name: usize = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_').map(char::len_utf8).sum();
    if name == 0 {
        return None;
    }
    rest[name..].strip_prefix('{')
}

fn keep<'a>(kept: &mut Vec<&'a str>, rest: &'a str) {
    if !rest.trim().is_empty() {
        kept.push(rest.trim());
    }
}

/// `text` in messages of at most `limit` characters. A cut falls on a blank line
/// between paragraphs, never inside a fenced code block. A block longer than a message
/// on its own is cut at line ends (a line longer than a message, at a character), and a
/// code block cut so is closed in one message and opened again in the next.
pub fn split(text: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    // The message being packed is `text[start..end]`; empty while nothing is.
    let (mut start, mut end) = (0, 0);
    for (from, to, fenced) in blocks(text) {
        if chars(&text[from..to]) > limit {
            if start < end {
                out.push(text[start..end].to_string());
            }
            (start, end) = (0, 0);
            out.extend(hard_split(&text[from..to], limit, fenced));
        } else {
            if start == end || chars(&text[start..to]) > limit {
                if start < end {
                    out.push(text[start..end].to_string());
                }
                start = from;
            }
            end = to;
        }
    }
    if start < end {
        out.push(text[start..end].to_string());
    }
    out
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

/// The blocks of `text`: `(start, end)` in bytes, without the newlines that follow, and
/// whether the block opens with a code fence. A paragraph ends at a blank line; blank
/// lines inside a code block belong to it.
fn blocks(text: &str) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    // An open paragraph: where it starts, and where its last line ends.
    let (mut para, mut para_end): (Option<usize>, usize) = (None, 0);
    let mut fence: Option<usize> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let blank = trimmed.trim().is_empty();
        let line_end = at + line.len();
        let content_end = line_end - usize::from(line.ends_with('\n'));
        let opens_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if let Some(from) = fence {
            if opens_fence {
                out.push((from, content_end, true));
                fence = None;
            }
        } else if opens_fence {
            if let Some(from) = para.take() {
                out.push((from, para_end, false));
            }
            fence = Some(at);
        } else if blank {
            if let Some(from) = para.take() {
                out.push((from, para_end, false));
            }
        } else if para.is_none() {
            para = Some(at);
        }
        if !blank {
            para_end = content_end;
        }
        at = line_end;
    }
    if let Some(from) = fence {
        out.push((from, text.trim_end().len(), true));
    } else if let Some(from) = para {
        out.push((from, text.trim_end().len(), false));
    }
    out
}

/// A block longer than `limit` on its own: cut at line ends, in messages that fit.
fn hard_split(block: &str, limit: usize, fenced: bool) -> Vec<String> {
    let (open, body) = match fenced {
        true => block.split_once('\n').unwrap_or((block, "")),
        false => ("", block),
    };
    let close: String = open.chars().take_while(|c| *c == '`' || *c == '~').collect();
    // Every piece carries the opening line, and all but the last a closing line too.
    let fences = if fenced { open.chars().count() + close.chars().count() + 2 } else { 0 };
    let room = limit.saturating_sub(fences).max(1);
    let lines: Vec<&str> = body.lines().flat_map(|line| chunks(line, room)).collect();
    let mut pieces: Vec<String> = Vec::new();
    let (mut piece, mut size) = (String::new(), 0usize);
    for line in lines {
        if size > 0 && size + 1 + chars(line) > room {
            pieces.push(std::mem::take(&mut piece));
            size = 0;
        }
        if size > 0 {
            piece.push('\n');
            size += 1;
        }
        piece.push_str(line);
        size += chars(line);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    let last = pieces.len().saturating_sub(1);
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, piece)| {
            let mut text = String::new();
            if fenced {
                text.push_str(open);
                text.push('\n');
            }
            text.push_str(&piece);
            if fenced && i < last {
                text.push('\n');
                text.push_str(&close);
            }
            text
        })
        .collect()
}

/// `line` as it is when it fits `room` characters, else in pieces of that many.
fn chunks(line: &str, room: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line;
    while chars(rest) > room {
        let end = rest.char_indices().nth(room).map_or(rest.len(), |(i, _)| i);
        out.push(&rest[..end]);
        rest = &rest[end..];
    }
    out.push(rest);
    out
}

mod html;
pub use html::{to_html, to_plain};
#[cfg(test)]
use html::escape;

#[cfg(test)]
mod tests;
