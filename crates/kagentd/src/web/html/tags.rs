//! The tag scanner of the HTML reader (`../html.rs`): where a tag starts and ends, its
//! attributes, and the range of an element. No DOM and no dependency: a `<` that opens
//! no tag is text.

use super::entities::decode_entities;

/// Raw text: nothing inside is markup, so they end at their first end tag.
const RAW: &[&str] = &["script", "style", "textarea"];
/// A tag as the walker reads it. `name` is empty for a comment, a doctype or a
/// processing instruction.
pub(super) struct Tag {
    /// Just after the `>`.
    /// Just after the `>`.
    pub(super) end: usize,
    pub(super) closing: bool,
    pub(super) self_closing: bool,
    pub(super) name: String,
}

/// The tag that starts at `at`, a `<`; `None` when the `<` is not the start of a tag.
pub(super) fn tag_at(html: &str, at: usize) -> Option<Tag> {
    let bogus = |end: usize| Some(Tag { end, closing: false, self_closing: false, name: String::new() });
    let mut cursor = at + 1;
    let first = html.as_bytes().get(cursor).copied()?;
    let mut closing = false;
    match first {
        b'!' => {
            if html[cursor..].starts_with("<!--") {
                cursor += 4;
                return bogus(html[cursor..].find("-->").map_or(html.len(), |offset| cursor + offset + 3));
            }
            return bogus(tag_end(html, cursor + 1));
        }
        b'?' => return bogus(tag_end(html, cursor + 1)),
        b'/' => {
            closing = true;
            cursor += 1;
            if !html.as_bytes().get(cursor).copied().is_some_and(|b| b.is_ascii_alphabetic()) {
                return bogus(tag_end(html, cursor));
            }
        }
        b if b.is_ascii_alphabetic() => {}
        _ => return None,
    }
    let start = cursor;
    while html.as_bytes().get(cursor).copied().is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':')) {
        cursor += 1;
    }
    let end = tag_end(html, cursor);
    Some(Tag { end, closing, self_closing: !closing && html.as_bytes()[end - 1] == b'/', name: html[start..cursor].to_ascii_lowercase() })
}

/// Just after the `>` that ends the tag whose markup starts at `from` (a quoted value may hold a `>`).
fn tag_end(html: &str, from: usize) -> usize {
    let mut quote = 0u8;
    for (offset, &b) in html.as_bytes()[from..].iter().enumerate() {
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            return from + offset + 1;
        }
    }
    html.len()
}

/// The attributes of a start tag (from its `<` to its `>`): names lowercased, values entity-decoded.
pub fn attrs_of(tag: &str) -> Vec<(String, String)> {
    let bytes = tag.as_bytes();
    let is_name = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':');
    let mut i = 1 + bytes.iter().skip(1).take_while(|b| is_name(**b)).count();
    let mut attrs = Vec::new();
    while i < bytes.len() {
        while bytes[i].is_ascii_whitespace() || bytes[i] == b'/' {
            i += 1;
            if i == bytes.len() {
                return attrs;
            }
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        if start == i {
            return attrs;
        }
        let name = tag[start..i].to_ascii_lowercase();
        let mut value = "";
        let mut after = i;
        while after < bytes.len() && bytes[after].is_ascii_whitespace() {
            after += 1;
        }
        if bytes.get(after) == Some(&b'=') {
            after += 1;
            while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                after += 1;
            }
            if matches!(bytes.get(after), Some(b'"') | Some(b'\'')) {
                let delimiter = bytes[after];
                after += 1;
                let from = after;
                while after < bytes.len() && bytes[after] != delimiter {
                    after += 1;
                }
                value = &tag[from..after];
                after += 1;
            } else {
                let from = after;
                while after < bytes.len() && !bytes[after].is_ascii_whitespace() && bytes[after] != b'>' {
                    after += 1;
                }
                value = &tag[from..after];
            }
            i = after;
        }
        attrs.push((name, decode_entities(value)));
    }
    attrs
}

/// The value of one attribute of a start tag.
pub(super) fn attr(tag: &str, name: &str) -> Option<String> {
    attrs_of(tag).into_iter().find(|(key, _)| key == name).map(|(_, value)| value)
}

/// The next tag named `name` from `from`: where it starts, just after it ends, and whether it closes.
pub(super) fn find_tag(html: &str, from: usize, name: &str) -> Option<(usize, usize, bool)> {
    let mut i = from;
    while let Some(offset) = html[i..].find('<') {
        let at = i + offset;
        match tag_at(html, at) {
            Some(tag) if tag.name == name => return Some((at, tag.end, tag.closing)),
            // Markup inside a script, a style or a textarea is text, not a tag.
            Some(tag) if matches!(tag.name.as_str(), "script" | "style" | "textarea") && !tag.closing && !tag.self_closing => {
                i = skip_element(html, tag.end, &tag.name)
            }
            Some(tag) => i = tag.end,
            None => i = at + 1,
        }
    }
    None
}

/// The content of the first `<name>` element of `html`: `None` when the page has none,
/// or never closes it (the whole page is read instead).
pub(super) fn element_range(html: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let (_, from, closing) = find_tag(html, 0, name)?;
    if closing {
        return None;
    }
    let mut depth = 1usize;
    let mut i = from;
    while let Some((at, after, closing)) = find_tag(html, i, name) {
        if closing {
            depth -= 1;
            if depth == 0 {
                return Some(from..at);
            }
        } else {
            depth += 1;
        }
        i = after;
    }
    None
}

/// Just after the element whose content starts at `from` (its matching end tag).
pub(super) fn skip_element(html: &str, from: usize, name: &str) -> usize {
    if RAW.contains(&name) {
        return find_tag(html, from, name).map_or(html.len(), |(_, end, _)| end);
    }
    let mut depth = 1usize;
    let mut i = from;
    while let Some((_, after, closing)) = find_tag(html, i, name) {
        if closing {
            depth -= 1;
            if depth == 0 {
                return after;
            }
        } else {
            depth += 1;
        }
        i = after;
    }
    html.len()
}

/// Just after `</head>`, or at `<body>` when the page never closes its head; where it
/// was, when the page has neither (a fragment).
pub(super) fn skip_head(html: &str, from: usize) -> usize {
    let mut i = from;
    while let Some(offset) = html[i..].find('<') {
        let at = i + offset;
        match tag_at(html, at) {
            Some(tag) if tag.name == "head" && tag.closing => return tag.end,
            Some(tag) if tag.name == "body" && !tag.closing => return at,
            Some(tag) => i = tag.end,
            None => i = at + 1,
        }
    }
    from
}
