//! What may be passed to git as a ref (diff.md section 4): checked before git sees it, so a value can never
//! be read as an option. Shared by the tickets (a `range` link) and the diff.

/// The longest ref accepted.
pub const MAX_REF: usize = 200;

/// Why `r` is not a ref KAgent passes to git, or `Ok`.
pub fn check_ref(r: &str) -> Result<(), String> {
    if r.is_empty() {
        return Err("a ref is empty".into());
    }
    if r.len() > MAX_REF {
        return Err(format!("a ref is at most {MAX_REF} characters"));
    }
    if r.starts_with('-') {
        return Err("a ref cannot start with '-'".into());
    }
    if let Some(c) = r.chars().find(|c| !(c.is_ascii_alphanumeric() || "._/~^@{}-".contains(*c))) {
        return Err(format!("a ref cannot contain {c:?}"));
    }
    if r.contains("..") {
        return Err("a ref cannot contain '..' (pass `from` and `to` separately)".into());
    }
    if r.contains("//") || r.ends_with('/') || r.ends_with(".lock") {
        return Err("not a valid ref name".into());
    }
    // `@{...}` only for the upstream forms and a number.
    let mut rest = r;
    while let Some(i) = rest.find("@{") {
        let tail = &rest[i + 2..];
        let Some(end) = tail.find('}') else { return Err("an unclosed '@{'".into()) };
        let inner = &tail[..end];
        if !(matches!(inner, "u" | "upstream" | "push") || (!inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()))) {
            return Err(format!("'@{{{inner}}}' is not accepted in a ref"));
        }
        rest = &tail[end + 1..];
    }
    Ok(())
}

/// A range as a ticket link holds it: `from..to`, or one ref.
pub fn check_range(range: &str) -> Result<(), String> {
    match range.split_once("..") {
        Some((from, to)) => {
            check_ref(from)?;
            check_ref(to)
        }
        None => check_ref(range),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_refs_pass_and_options_never_do() {
        for ok in ["HEAD", "HEAD~3", "main", "v0.0.3", "origin/master", "abc1234", "HEAD^", "@{u}", "feature/x-1", "main@{1}"] {
            assert!(check_ref(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "--output=/tmp/x", "a b", "a;b", "a..b", "x\n", "a$(b)", "@{evil}", "a//b", "a/", "x.lock", "a:b", "a\\b"] {
            assert!(check_ref(bad).is_err(), "{bad:?}");
        }
        assert!(check_ref(&"a".repeat(201)).is_err());
    }

    #[test]
    fn ranges() {
        assert!(check_range("master..v0.0.3").is_ok());
        assert!(check_range("abc1234").is_ok());
        assert!(check_range("-a..b").is_err());
        assert!(check_range("a..-b").is_err());
    }
}
