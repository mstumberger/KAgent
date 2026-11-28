//! The line diff (diff.md section 2): in process, no dependency. The common prefix and suffix are cut off, a longest
//! common subsequence by dynamic programming runs over the middle (at most 4 million cells; a larger middle is all
//! removed, then all added), and the changes are grouped into hunks with three lines of context.

use super::{FileDiff, Hunk, Kind, Line, Status, CONTEXT, MAX_FILE_BYTES};

/// Cells of the LCS table at most.
const MAX_CELLS: usize = 4_000_000;

/// The lines of a text: split at `\n`, a `\r` before it is not part of the line, and a final newline ends the last
/// line instead of starting an empty one.
pub fn split(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Keep,
    Del,
    Add,
}

/// The edit script from `a` to `b`: one op per line of either, in order.
fn script(a: &[&str], b: &[&str]) -> Vec<Op> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (ma, mb) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut ops = vec![Op::Keep; prefix];
    if ma.len().saturating_mul(mb.len()) > MAX_CELLS {
        ops.extend(std::iter::repeat_n(Op::Del, ma.len()));
        ops.extend(std::iter::repeat_n(Op::Add, mb.len()));
    } else {
        // lcs[i][j]: the length of the longest common subsequence of ma[i..] and mb[j..].
        let w = mb.len() + 1;
        let mut lcs = vec![0u32; (ma.len() + 1) * w];
        for i in (0..ma.len()).rev() {
            for j in (0..mb.len()).rev() {
                lcs[i * w + j] = if ma[i] == mb[j] { lcs[(i + 1) * w + j + 1] + 1 } else { lcs[(i + 1) * w + j].max(lcs[i * w + j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < ma.len() || j < mb.len() {
            if i < ma.len() && j < mb.len() && ma[i] == mb[j] {
                ops.push(Op::Keep);
                i += 1;
                j += 1;
            } else if j >= mb.len() || (i < ma.len() && lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
                ops.push(Op::Del);
                i += 1;
            } else {
                ops.push(Op::Add);
                j += 1;
            }
        }
    }
    ops.extend(std::iter::repeat_n(Op::Keep, suffix));
    ops
}

/// The characters that differ between two lines: `[start, end)` in each, after the common prefix and suffix; `None` when
/// the lines are equal.
fn differing(a: &str, b: &str) -> Option<([usize; 2], [usize; 2])> {
    let (x, y): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let prefix = x.iter().zip(&y).take_while(|(p, q)| p == q).count();
    let suffix = x[prefix..].iter().rev().zip(y[prefix..].iter().rev()).take_while(|(p, q)| p == q).count();
    let (ra, rb) = ([prefix, x.len() - suffix], [prefix, y.len() - suffix]);
    (ra[0] != ra[1] || rb[0] != rb[1]).then_some((ra, rb))
}

/// Intra-line ranges on the changed lines of a hunk: in each block of removed lines followed by added lines, the first
/// removed pairs with the first added, and so on.
pub fn mark_ranges(lines: &mut [Line]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != Kind::Del {
            i += 1;
            continue;
        }
        let dels: Vec<usize> = (i..lines.len()).take_while(|&k| lines[k].kind == Kind::Del).collect();
        let start_add = i + dels.len();
        let adds: Vec<usize> = (start_add..lines.len()).take_while(|&k| lines[k].kind == Kind::Add).collect();
        for (d, a) in dels.iter().zip(&adds) {
            if let Some((rd, ra)) = differing(&lines[*d].text, &lines[*a].text) {
                if rd[0] != rd[1] {
                    lines[*d].ranges = vec![rd];
                }
                if ra[0] != ra[1] {
                    lines[*a].ranges = vec![ra];
                }
            }
        }
        i = start_add + adds.len();
    }
}

/// The hunks of the change from `old` to `new`, and the counts of added and removed lines.
pub fn hunks(old: &str, new: &str) -> (Vec<Hunk>, usize, usize) {
    let (a, b) = (split(old), split(new));
    let ops = script(&a, &b);
    // Every op with its line numbers (one-based, on the side(s) it has).
    let mut rows: Vec<Line> = Vec::with_capacity(ops.len());
    let (mut i, mut j) = (0, 0);
    for op in &ops {
        rows.push(match op {
            Op::Keep => {
                i += 1;
                j += 1;
                Line { kind: Kind::Ctx, old: Some(i), new: Some(j), text: a[i - 1].to_string(), ranges: Vec::new() }
            }
            Op::Del => {
                i += 1;
                Line { kind: Kind::Del, old: Some(i), new: None, text: a[i - 1].to_string(), ranges: Vec::new() }
            }
            Op::Add => {
                j += 1;
                Line { kind: Kind::Add, old: None, new: Some(j), text: b[j - 1].to_string(), ranges: Vec::new() }
            }
        });
    }
    let added = ops.iter().filter(|o| **o == Op::Add).count();
    let removed = ops.iter().filter(|o| **o == Op::Del).count();
    // Group the changes: a change and the context around it; two groups closer than twice the context are one.
    let changed: Vec<usize> = (0..rows.len()).filter(|&k| rows[k].kind != Kind::Ctx).collect();
    let mut hunks = Vec::new();
    let mut at = 0;
    while at < changed.len() {
        let mut end = at;
        while end + 1 < changed.len() && changed[end + 1] - changed[end] <= 2 * CONTEXT + 1 {
            end += 1;
        }
        let from = changed[at].saturating_sub(CONTEXT);
        let to = (changed[end] + CONTEXT + 1).min(rows.len());
        let mut lines: Vec<Line> = rows[from..to].to_vec();
        mark_ranges(&mut lines);
        let old_lines = lines.iter().filter(|l| l.old.is_some()).count();
        let new_lines = lines.iter().filter(|l| l.new.is_some()).count();
        let old_start = lines.iter().find_map(|l| l.old).unwrap_or(0);
        let new_start = lines.iter().find_map(|l| l.new).unwrap_or(0);
        // An empty side starts at the line before, as git writes it.
        let (old_start, new_start) = (if old_lines == 0 { old_start_before(&rows, from) } else { old_start }, if new_lines == 0 { new_start_before(&rows, from) } else { new_start });
        hunks.push(Hunk { old_start, old_lines, new_start, new_lines, header: String::new(), lines });
        at = end + 1;
    }
    (hunks, added, removed)
}

fn old_start_before(rows: &[Line], from: usize) -> usize {
    rows[..from].iter().rev().find_map(|l| l.old).unwrap_or(0)
}

fn new_start_before(rows: &[Line], from: usize) -> usize {
    rows[..from].iter().rev().find_map(|l| l.new).unwrap_or(0)
}

/// The diff of one file from its two texts. A text with a NUL byte is binary; one over 1 MB is too large: a summary only.
pub fn diff_texts(path: &str, old: &str, new: &str, status: Status) -> FileDiff {
    let mut file = FileDiff::summary(path, status);
    if old.contains('\0') || new.contains('\0') {
        file.binary = true;
        file.added = None;
        file.removed = None;
        return file;
    }
    if old.len() > MAX_FILE_BYTES || new.len() > MAX_FILE_BYTES {
        file.too_large = true;
        file.added = None;
        file.removed = None;
        return file;
    }
    let (hunks, added, removed) = hunks(old, new);
    file.hunks = hunks;
    file.added = Some(added);
    file.removed = Some(removed);
    file
}
