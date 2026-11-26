//! The entities of HTML (web.md section 2): the named ones a page commonly has, and
//! the numeric forms.

use super::charset::CP1252;

/// The named entities decoded, besides the numeric forms.
const NAMED: &[(&str, char)] = &[
    ("amp", '&'), ("lt", '<'), ("gt", '>'), ("quot", '"'), ("apos", '\''), ("nbsp", ' '),
    ("ndash", '\u{2013}'), ("mdash", '\u{2014}'), ("hellip", '\u{2026}'), ("rsquo", '\u{2019}'), ("lsquo", '\u{2018}'),
    ("ldquo", '\u{201C}'), ("rdquo", '\u{201D}'), ("sbquo", '\u{201A}'), ("bdquo", '\u{201E}'), ("laquo", '\u{AB}'),
    ("raquo", '\u{BB}'), ("copy", '\u{A9}'), ("reg", '\u{AE}'), ("trade", '\u{2122}'), ("deg", '\u{B0}'), ("plusmn", '\u{B1}'),
    ("times", '\u{D7}'), ("divide", '\u{F7}'), ("euro", '\u{20AC}'), ("pound", '\u{A3}'), ("yen", '\u{A5}'), ("cent", '\u{A2}'),
    ("sect", '\u{A7}'), ("para", '\u{B6}'), ("middot", '\u{B7}'), ("bull", '\u{2022}'), ("dagger", '\u{2020}'), ("Dagger", '\u{2021}'),
    ("permil", '\u{2030}'), ("prime", '\u{2032}'), ("Prime", '\u{2033}'), ("larr", '\u{2190}'), ("uarr", '\u{2191}'), ("rarr", '\u{2192}'),
    ("darr", '\u{2193}'), ("harr", '\u{2194}'), ("ne", '\u{2260}'), ("le", '\u{2264}'), ("ge", '\u{2265}'), ("minus", '\u{2212}'),
    ("lowast", '\u{2217}'), ("infin", '\u{221E}'), ("szlig", '\u{DF}'), ("emsp", ' '), ("ensp", ' '), ("thinsp", ' '),
    ("shy", '\u{AD}'), ("zwnj", '\u{200C}'), ("zwj", '\u{200D}'), ("iexcl", '\u{A1}'), ("iquest", '\u{BF}'), ("micro", '\u{B5}'),
    ("sup1", '\u{B9}'), ("sup2", '\u{B2}'), ("sup3", '\u{B3}'), ("frac12", '\u{BD}'), ("frac14", '\u{BC}'), ("frac34", '\u{BE}'),
    ("uml", '\u{A8}'), ("not", '\u{AC}'), ("macr", '\u{AF}'), ("acute", '\u{B4}'), ("cedil", '\u{B8}'), ("ordf", '\u{AA}'),
    ("ordm", '\u{BA}'), ("curren", '\u{A4}'), ("brvbar", '\u{A6}'),
    ("AElig", '\u{C6}'), ("Aacute", '\u{C1}'), ("Acirc", '\u{C2}'), ("Agrave", '\u{C0}'), ("Aring", '\u{C5}'), ("Atilde", '\u{C3}'),
    ("Auml", '\u{C4}'), ("Ccedil", '\u{C7}'), ("ETH", '\u{D0}'), ("Eacute", '\u{C9}'), ("Ecirc", '\u{CA}'), ("Egrave", '\u{C8}'),
    ("Euml", '\u{CB}'), ("Iacute", '\u{CD}'), ("Icirc", '\u{CE}'), ("Igrave", '\u{CC}'), ("Iuml", '\u{CF}'), ("Ntilde", '\u{D1}'),
    ("Oacute", '\u{D3}'), ("Ocirc", '\u{D4}'), ("Ograve", '\u{D2}'), ("Oslash", '\u{D8}'), ("Otilde", '\u{D5}'), ("Ouml", '\u{D6}'),
    ("THORN", '\u{DE}'), ("Uacute", '\u{DA}'), ("Ucirc", '\u{DB}'), ("Ugrave", '\u{D9}'), ("Uuml", '\u{DC}'), ("Yacute", '\u{DD}'),
    ("aacute", '\u{E1}'), ("acirc", '\u{E2}'), ("aelig", '\u{E6}'), ("agrave", '\u{E0}'), ("aring", '\u{E5}'), ("atilde", '\u{E3}'),
    ("auml", '\u{E4}'), ("ccedil", '\u{E7}'), ("eacute", '\u{E9}'), ("ecirc", '\u{EA}'), ("egrave", '\u{E8}'),
    ("eth", '\u{F0}'), ("euml", '\u{EB}'), ("iacute", '\u{ED}'), ("icirc", '\u{EE}'), ("igrave", '\u{EC}'), ("iuml", '\u{EF}'),
    ("ntilde", '\u{F1}'), ("oacute", '\u{F3}'), ("ocirc", '\u{F4}'), ("ograve", '\u{F2}'), ("oslash", '\u{F8}'), ("otilde", '\u{F5}'),
    ("ouml", '\u{F6}'), ("thorn", '\u{FE}'), ("uacute", '\u{FA}'), ("ucirc", '\u{FB}'), ("ugrave", '\u{F9}'), ("uuml", '\u{FC}'),
    ("yacute", '\u{FD}'), ("yuml", '\u{FF}'),
];

/// The entity, when `name` (between `&` and `;`) is one the page can mean. A numeric
/// form in the range of the old controls reads as its windows-1252 character, as in a browser.
fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse::<u32>().ok()?,
        };
        return match code {
            0 => Some('\u{FFFD}'),
            0x80..=0x9f => char::from_u32(u32::from(CP1252[code as usize - 0x80])),
            other => char::from_u32(other),
        };
    }
    NAMED.iter().find(|(known, _)| *known == name).map(|(_, character)| *character)
}

/// The entities of a text node or an attribute value decoded. An unknown one (`&nope;`) and a bare `&` stay as they are.
pub fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        // The name ends at the next `;`, when that is near enough to belong to it.
        // The name is ASCII letters and digits (after a `#` for a numeric form), and short.
        let name = |end: usize| after[..end].chars().all(|c| c.is_ascii_alphanumeric() || (c == '#' && end > 1));
        rest = match after.find(';').filter(|end| *end <= 10 && name(*end)) {
            Some(end) => {
                match entity(&after[..end]) {
                    Some(character) => out.push(character),
                    None => {
                        out.push('&');
                        out.push_str(&after[..=end]);
                    }
                }
                &after[end + 1..]
            }
            None => {
                out.push('&');
                after
            }
        };
    }
    out.push_str(rest);
    out
}
