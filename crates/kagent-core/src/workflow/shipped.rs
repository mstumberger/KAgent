//! The workflows that come with KAgent (workflows.md section 2): the files of `workflows/` in the source tree, built in,
//! read-only. A file of the config folder cannot use their names.

use super::definition::Definition;

const FILES: [(&str, &str); 4] = [
    ("plan-implement-review", include_str!("../../../../workflows/plan-implement-review.yaml")),
    ("implement-review", include_str!("../../../../workflows/implement-review.yaml")),
    ("plan-only", include_str!("../../../../workflows/plan-only.yaml")),
    ("review-only", include_str!("../../../../workflows/review-only.yaml")),
];

pub fn names() -> Vec<&'static str> {
    FILES.iter().map(|(n, _)| *n).collect()
}

pub fn is_shipped(name: &str) -> bool {
    FILES.iter().any(|(n, _)| *n == name)
}

/// The shipped definitions. They are checked by the tests, so a failure here is a bug of the build.
pub fn all(known_profile: &dyn Fn(&str) -> bool) -> Vec<Definition> {
    FILES.iter().filter_map(|(name, text)| Definition::parse(text, name, known_profile).map_err(|e| tracing::error!("shipped workflow {name}: {e}")).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_workflow_is_valid() {
        let known = |p: &str| super::super::roles::profile(p).is_some();
        for (name, text) in FILES {
            let d = Definition::parse(text, name, &known).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(d.name, name);
        }
        assert_eq!(all(&known).len(), 4);
    }
}
