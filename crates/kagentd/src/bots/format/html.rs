//! Markdown as Telegram HTML (bots.md section 3): `<`, `>` and `&` escaped; bold,
//! italic and code kept; headings, tables and links as plain text. The second half of
//! `format.rs`; the tests are in `format/tests.rs`.

pub fn to_html(markdown: &str) -> String {
    render(markdown, true)
}

/// The same text with no tag at all, escaped: sent when Telegram will not parse the HTML.
pub fn to_plain(markdown: &str) -> String {
    render(markdown, false)
}

fn render(markdown: &str, tags: bool) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut code = false;
    for line in markdown.lines() {
        let start = line.trim_start();
        if start.starts_with("```") || start.starts_with("~~~") {
            if code {
                // The newline of the last code line is not part of the block.
                if out.ends_with('\n') {
                    out.pop();
                }
                if tags {
                    out.push_str("</code></pre>");
                }
                out.push('\n');
            } else if tags {
                out.push_str("<pre><code>");
            } else {
                out.push('\n');
            }
            code = !code;
            continue;
        }
        if code {
            out.push_str(&escape(line));
        } else if is_table_rule(start) {
            // The rule row of a table says nothing.
            continue;
        } else {
            out.push_str(&inline(table_edges(heading(line)), tags));
        }
        out.push('\n');
    }
    if code && tags {
        if out.ends_with('\n') {
            out.pop();
        }
        out.push_str("</code></pre>");
    }
    out.trim().to_string()
}

/// `<`, `>` and `&` as Telegram wants them in text.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        }
    }
    out
}

/// The line between the rows of a table: `| --- | :---: |`.
fn is_table_rule(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.contains('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// A heading without its markers: `## Title` is `Title`.
fn heading(line: &str) -> &str {
    let t = line.trim_start();
    let marks = t.chars().take_while(|c| *c == '#').count();
    let body = if (1..=6).contains(&marks) && t[marks..].starts_with(' ') { &t[marks + 1..] } else { t };
    body.trim_end().trim_end_matches('#').trim_end()
}

/// A row of a table without its outer pipes: `| a | b |` is `a | b`.
fn table_edges(line: &str) -> &str {
    let t = line.trim();
    t.strip_prefix('|').and_then(|t| t.strip_suffix('|')).map(str::trim).filter(|inner| inner.contains('|')).unwrap_or(t)
}

/// One line without block markers: code spans, bold, italic and links. A tag is put
/// around one run of text at a time, open and closed together, so the line's tags are
/// always balanced. A marker that finds no partner in the line is text, as it is.
fn inline(text: &str, tags: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let (mut out, mut run) = (String::new(), String::new());
    // The marker of each span that is open: bold, italic.
    let (mut bold, mut italic) = (None, None);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let ahead = |n: usize| chars.get(i + n).copied();
        let before = if i == 0 { None } else { Some(chars[i - 1]) };
        if c == '\\' && ahead(1).is_some_and(|n| n.is_ascii_punctuation()) {
            // `\*` is a star, not a marker.
            run.push(chars[i + 1]);
            i += 2;
        } else if c == '`' {
            let n = run_len(&chars, i);
            match code_span(&chars, i + n, n) {
                Some((end, _)) => {
                    flush(&mut out, &mut run, bold, italic, tags);
                    if tags {
                        out.push_str("<code>");
                    }
                    out.push_str(&escape(&chars[i + n..end].iter().collect::<String>()));
                    if tags {
                        out.push_str("</code>");
                    }
                    i = end + n;
                }
                None => {
                    run.push(c);
                    i += 1;
                }
            }
        } else if (c == '*' || c == '_') && ahead(1) == Some(c) {
            // The text before the marker goes in the tags open so far; then they change.
            let next = if bold == Some(c) {
                None
            } else if bold.is_none() && !ahead(2).is_none_or(|n| n.is_whitespace()) {
                Some(c)
            } else {
                run.push(c);
                run.push(c);
                bold
            };
            flush(&mut out, &mut run, bold, italic, tags);
            bold = next;
            i += 2;
        } else if c == '*' || c == '_' {
            // `_` only opens or closes between words: `file_name` stays as it is.
            let between_words = c == '*' || before.is_none_or(|b| b.is_whitespace() || b.is_ascii_punctuation());
            let next_blank = ahead(1).is_none_or(|n| n.is_whitespace());
            let next = if italic == Some(c) && !before.is_some_and(|b| b.is_whitespace()) {
                None
            } else if italic.is_none() && !next_blank && between_words {
                Some(c)
            } else {
                run.push(c);
                italic
            };
            flush(&mut out, &mut run, bold, italic, tags);
            italic = next;
            i += 1;
        } else if c == '[' {
            match link(&chars, i) {
                Some((label, url, end)) => {
                    flush(&mut out, &mut run, bold, italic, tags);
                    if label.is_empty() || label == url {
                        out.push_str(&escape(&url));
                    } else {
                        out.push_str(&escape(&label));
                        out.push_str(" (");
                        out.push_str(&escape(&url));
                        out.push(')');
                    }
                    i = end;
                }
                None => {
                    run.push(c);
                    i += 1;
                }
            }
        } else {
            run.push(c);
            i += 1;
        }
    }
    flush(&mut out, &mut run, bold, italic, tags);
    out
}

/// The text since the last marker, escaped, in the tags that are open.
fn flush(out: &mut String, run: &mut String, bold: Option<char>, italic: Option<char>, tags: bool) {
    if run.is_empty() {
        return;
    }
    if tags {
        if bold.is_some() {
            out.push_str("<b>");
        }
        if italic.is_some() {
            out.push_str("<i>");
        }
    }
    out.push_str(&escape(run));
    if tags {
        if italic.is_some() {
            out.push_str("</i>");
        }
        if bold.is_some() {
            out.push_str("</b>");
        }
    }
    run.clear();
}

/// `[label](url)` from `[`: the two, and where the link ends.
fn link(chars: &[char], at: usize) -> Option<(String, String, usize)> {
    let close = (at + 1..chars.len()).find(|&j| chars[j] == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = (close + 2..chars.len()).find(|&j| chars[j] == ')')?;
    Some((chars[at + 1..close].iter().collect(), chars[close + 2..end].iter().collect::<String>().trim().to_string(), end + 1))
}

fn run_len(chars: &[char], at: usize) -> usize {
    chars[at..].iter().take_while(|c| **c == '`').count()
}

/// A run of `n` backticks from `from` on: where it starts.
fn code_span(chars: &[char], from: usize, n: usize) -> Option<(usize, usize)> {
    let mut j = from;
    while j < chars.len() {
        if chars[j] == '`' {
            let m = run_len(chars, j);
            if m == n {
                return Some((j, m));
            }
            j += m;
        } else {
            j += 1;
        }
    }
    None
}
