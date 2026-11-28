//! HTML to text (web.md section 2), written by hand: no dependency and no DOM. The
//! result is a compact page for the model, not a rendering. `script`, `style`,
//! `noscript`, `template`, `svg` and `iframe` are dropped; a `<main>` (else the first
//! `<article>`) is the page when there is one; block elements end lines; whitespace is
//! collapsed and the common entities are decoded. A body that is not UTF-8 is read by
//! the charset it declares.

mod charset;
mod entities;
mod tags;

use reqwest::Url;

pub use charset::decode;
pub use entities::decode_entities;
pub use tags::attrs_of;
use tags::{attr, element_range, find_tag, skip_element, skip_head, tag_at};

/// The most links a page gives (web.md section 2).
pub const MAX_LINKS: usize = 30;
/// Characters of the text of a link.
const LINK_TEXT: usize = 80;

/// Elements that end a line before and after them.
const BLOCK: &[&str] = &[
    "address", "article", "aside", "blockquote", "caption", "dd", "details", "dialog", "div", "dl", "dt", "fieldset", "figcaption", "figure", "footer", "form",
    "h1", "h2", "h3", "h4", "h5", "h6", "header", "li", "main", "nav", "ol", "option", "p", "pre", "section", "summary", "table", "tbody", "tfoot", "thead",
    "tr", "ul",
];
/// Table cells: the row stays on one line, a space between the cells.
const CELLS: &[&str] = &["td", "th"];
/// Dropped with their content (web.md section 2), and the title when it is not in `<head>`.
const DROPPED: &[&str] = &["script", "style", "noscript", "template", "svg", "iframe", "title", "textarea"];

/// What `to_text` found in a page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub title: String,
    pub text: String,
    /// The text of each link and its address, in order, at most `MAX_LINKS`.
    pub links: Vec<(String, String)>,
    /// Set by the fetcher, not by `to_text`: the page is a challenge or an empty shell that needs
    /// JavaScript, and the reader should read it (web.md section 2).
    pub needs_reader: bool,
}

/// The page as text. `base` is the address the HTML came from: links are resolved against it.
pub fn to_text(html: &str, base: &Url) -> Page {
    let mut page = Page { title: title_of(html), ..Page::default() };
    if let Some(range) = element_range(html, "main").or_else(|| element_range(html, "article")) {
        walk(&html[range], base, &mut page);
        // A main that says nothing (it holds a frame or a script) is not the page: the whole of it is read.
        if !page.text.is_empty() || !page.links.is_empty() {
            return page;
        }
    }
    walk(html, base, &mut page);
    page
}

/// Text being collected: whitespace is collapsed and a line does not start with a space.
#[derive(Default)]
struct Text {
    out: String,
    /// A block ended: the line breaks when more text comes.
    break_pending: bool,
    /// Whitespace was met since the last word (a space follows the next one).
    pending_space: bool,
}

impl Text {
    /// A text node, entities decoded. Two nodes that meet without whitespace between
    /// them stay joined (`<b>a</b>,` is `a,`), as in the page.
    fn push(&mut self, text: &str) {
        if self.break_pending {
            self.newline();
            self.break_pending = false;
        }
        let mut space = self.pending_space || text.starts_with(char::is_whitespace);
        for word in text.split_whitespace() {
            if !self.out.is_empty() && !self.out.ends_with('\n') && space {
                self.out.push(' ');
            }
            self.out.push_str(word);
            space = true;
        }
        if let Some(last) = text.chars().last() {
            self.pending_space = last.is_whitespace();
        }
    }

    /// The line ends here and now (`<br>`, `<hr>`).
    fn newline(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }
}

/// The markup of `html`, which is the content of the page (web.md section 2).
fn walk(html: &str, base: &Url, page: &mut Page) {
    let mut text = Text::default();
    // The link that is open: where its text starts in `text.out`, where its markup
    // starts in `html` (an image may stand in for text), and its `href`.
    let mut link: Option<(usize, usize, String)> = None;
    let close_link = |link: &mut Option<(usize, usize, String)>, html: &str, to: usize, text: &Text, links: &mut Vec<(String, String)>| {
        if let Some((mark, from, href)) = link.take() {
            let mut label = collapse(&text.out[mark..]);
            if label.is_empty() {
                label = image_alt(&html[from..to]).unwrap_or_default();
            }
            add_link(links, &label, &href, base);
        }
    };
    let mut i = 0;
    while let Some(offset) = html[i..].find('<') {
        let at = i + offset;
        text.push(&decode_entities(&html[i..at]));
        let Some(tag) = tag_at(html, at) else {
            text.push("<");
            i = at + 1;
            continue;
        };
        i = tag.end;
        let name = tag.name.as_str();
        if tag.closing {
            if name == "a" {
                close_link(&mut link, html, at, &text, &mut page.links);
            }
            if BLOCK.contains(&name) {
                text.break_pending = true;
            } else if CELLS.contains(&name) {
                text.pending_space = true;
            }
            continue;
        }
        if DROPPED.contains(&name) && !tag.self_closing {
            i = skip_element(html, i, name);
        } else if name == "head" && !tag.self_closing {
            i = skip_head(html, i);
        } else {
            if name == "br" || name == "hr" {
                text.newline();
            } else if BLOCK.contains(&name) {
                text.break_pending = true;
            }
            if name == "a" {
                if let Some(href) = attr(&html[at..tag.end], "href") {
                    link = Some((text.out.len(), tag.end, href));
                }
            }
        }
    }
    text.push(&decode_entities(&html[i..]));
    close_link(&mut link, html, html.len(), &text, &mut page.links);
    page.text = text.out.trim().to_string();
}

/// The `alt` of the first image of a link that has no text of its own (a badge, an icon).
fn image_alt(inner: &str) -> Option<String> {
    let mut i = 0;
    while let Some(offset) = inner[i..].find('<') {
        let at = i + offset;
        match tag_at(inner, at) {
            Some(tag) if tag.name == "img" && !tag.closing => {
                return attr(&inner[at..tag.end], "alt").map(|alt| collapse(&alt)).filter(|alt| !alt.is_empty());
            }
            Some(tag) => i = tag.end,
            None => i = at + 1,
        }
    }
    None
}

/// One link, when it is one: http or https, with a text; not a fragment of the page, not one listed before.
fn add_link(links: &mut Vec<(String, String)>, label: &str, href: &str, base: &Url) {
    if links.len() >= MAX_LINKS || label.is_empty() {
        return;
    }
    let Some(url) = resolve(base, href) else { return };
    if links.iter().any(|(_, listed)| *listed == url) {
        return;
    }
    links.push((cut(&collapse(label), LINK_TEXT), url));
}

/// The address of `href` against the page's own. `None` for an empty one, a fragment of
/// the page itself, or another scheme (`mailto:`, `javascript:`).
pub fn resolve(base: &Url, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with('#') {
        return None;
    }
    let url = base.join(href).ok()?;
    (url.scheme() == "http" || url.scheme() == "https").then(|| url.to_string())
}

/// `s` with at most `max` characters; `...` says it was longer.
pub fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => format!("{}...", &s[..end]),
        None => s.to_string(),
    }
}

/// The text of a fragment of markup: tags and comments removed, entities decoded, whitespace collapsed.
pub fn strip_tags(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut i = 0;
    while let Some(offset) = fragment[i..].find('<') {
        out.push_str(&fragment[i..i + offset]);
        match tag_at(fragment, i + offset) {
            Some(tag) => i = tag.end,
            None => {
                out.push('<');
                i += offset + 1;
            }
        }
    }
    out.push_str(&fragment[i..]);
    collapse(&decode_entities(&out))
}

/// Runs of whitespace become one space; none at the ends.
pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The title of the page: the text of its `<title>`, wherever the page keeps it.
fn title_of(html: &str) -> String {
    let Some((_, from, _)) = find_tag(html, 0, "title") else { return String::new() };
    let to = html[from..].find('<').map_or(html.len(), |offset| from + offset);
    collapse(&decode_entities(&html[from..to]))
}

#[cfg(test)]
mod tests;
