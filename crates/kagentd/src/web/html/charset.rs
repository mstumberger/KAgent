//! The charset of a body that is not UTF-8 (web.md section 2): the two Latin families
//! a page of this part of the world still declares, tables generated from Python's
//! codecs; any other charset reads a byte that is not valid as U+FFFD.

use super::super::charset_of;

/// How far into a body its `<meta charset>` is looked for.
const META_WINDOW: usize = 4096;
/// Windows-1252: what a page means by `iso-8859-1` (the 5 bytes it leaves open read as themselves).
pub(super) const CP1252: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    0x00A0, 0x00A1, 0x00A2, 0x00A3, 0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB, 0x00BC, 0x00BD, 0x00BE, 0x00BF,
    0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7, 0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF,
    0x00D0, 0x00D1, 0x00D2, 0x00D3, 0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB, 0x00EC, 0x00ED, 0x00EE, 0x00EF,
    0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7, 0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];
/// Windows-1250: Central European Latin (Croatian, Hungarian, Polish, Slovenian...).
const CP1250: [u16; 128] = [
    0x20AC, 0x0081, 0x201A, 0x0083, 0x201E, 0x2026, 0x2020, 0x2021, 0x0088, 0x2030, 0x0160, 0x2039, 0x015A, 0x0164, 0x017D, 0x0179,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x0098, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A,
    0x00A0, 0x02C7, 0x02D8, 0x0141, 0x00A4, 0x0104, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x0105, 0x015F, 0x00BB, 0x013D, 0x02DD, 0x013E, 0x017C,
    0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7, 0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E,
    0x0110, 0x0143, 0x0147, 0x00D3, 0x00D4, 0x0150, 0x00D6, 0x00D7, 0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7, 0x010D, 0x00E9, 0x0119, 0x00EB, 0x011B, 0x00ED, 0x00EE, 0x010F,
    0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7, 0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

/// The table of a charset of the Latin family, by the name a page gives it.
fn latin_table(charset: &str) -> Option<&'static [u16; 128]> {
    match charset.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "iso-8859-1" | "iso8859-1" | "latin1" | "latin-1" | "l1" | "ibm819" | "cp819" | "cp1252" | "windows-1252" | "x-cp1252"
        | "iso-8859-15" | "latin9" | "iso8859-15" => Some(&CP1252),
        "iso-8859-2" | "iso8859-2" | "latin2" | "latin-2" | "l2" | "cp1250" | "windows-1250" | "x-cp1250" => Some(&CP1250),
        _ => None,
    }
}

/// The text of a body. UTF-8 when it is; else by the charset the `Content-Type` names,
/// else by the one the page declares in its first 4 KiB. Windows-1252 and windows-1250
/// are decoded; in any other charset a byte that is not valid reads as U+FFFD.
pub fn decode(bytes: &[u8], content_type: Option<&str>) -> String {
    match bytes {
        [0xef, 0xbb, 0xbf, rest @ ..] => return String::from_utf8_lossy(rest).into_owned(),
        [0xff, 0xfe, rest @ ..] if rest.len() % 2 == 0 => return utf16(rest, true),
        [0xfe, 0xff, rest @ ..] if rest.len() % 2 == 0 => return utf16(rest, false),
        _ => {}
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let declared = content_type.and_then(charset_of).or_else(|| meta_charset(bytes));
    match declared.as_deref().and_then(latin_table) {
        Some(table) => bytes.iter().map(|&b| latin_byte(b, table)).collect(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn latin_byte(b: u8, table: &[u16; 128]) -> char {
    if b < 0x80 {
        b as char
    } else {
        char::from_u32(u32::from(table[usize::from(b - 0x80)])).unwrap_or('\u{FFFD}')
    }
}

/// UTF-16 with its byte order known from the mark; an unpaired half of a pair reads as U+FFFD.
fn utf16(bytes: &[u8], little: bool) -> String {
    let units = bytes.chunks_exact(2).map(|pair| if little { u16::from_le_bytes([pair[0], pair[1]]) } else { u16::from_be_bytes([pair[0], pair[1]]) });
    char::decode_utf16(units).map(|unit| unit.unwrap_or('\u{FFFD}')).collect()
}

/// The charset a page declares: `<meta charset="...">`, or in the `content` of a `<meta http-equiv>`.
fn meta_charset(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(META_WINDOW)]).to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = head[from..].find("<meta") {
        let start = from + at;
        let end = head[start..].find('>').map_or(head.len(), |offset| start + offset);
        if let Some(value) = charset_of(&head[start..end]) {
            return Some(value);
        }
        from = end;
    }
    None
}
