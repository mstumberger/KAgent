//! The verdict a reviewer must return (workflows.md section 3.3).

use serde::Deserialize;

use super::model::{Finding, Severity, Verdict};
use super::plan::json_block;

#[derive(Deserialize)]
struct Raw {
    verdict: Verdict,
    #[serde(default)]
    findings: Vec<Finding>,
}

/// The verdict in `text`: `changes` needs a `must` finding, `pass` has none. `Err` says what to put right.
pub fn parse(text: &str) -> Result<(Verdict, Vec<Finding>), String> {
    let Some(block) = json_block(text) else {
        return Err("there is no fenced ```json block in your last message".into());
    };
    let raw: Raw = serde_json::from_str(&block).map_err(|e| format!("the json block is not a verdict ({{\"verdict\": \"pass\" | \"changes\", \"findings\": [...]}}): {e}"))?;
    let musts = raw.findings.iter().filter(|f| f.severity == Severity::Must).count();
    if raw.findings.iter().any(|f| f.what.trim().is_empty()) {
        return Err("every finding needs `what`".into());
    }
    match raw.verdict {
        Verdict::Changes if musts == 0 => Err("`changes` needs at least one finding with severity `must`; if nothing must change, the verdict is `pass`".into()),
        Verdict::Pass if musts > 0 => Err("`pass` cannot have a `must` finding: say `changes`, or make the finding `should`".into()),
        v => Ok((v, raw.findings)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(json: &str) -> String {
        format!("review done\n```json\n{json}\n```")
    }

    #[test]
    fn pass_and_changes() {
        assert_eq!(parse(&block("{\"verdict\":\"pass\",\"findings\":[]}")).unwrap(), (Verdict::Pass, vec![]));
        let (v, f) = parse(&block("{\"verdict\":\"changes\",\"findings\":[{\"severity\":\"must\",\"file\":\"a.rs\",\"line\":3,\"what\":\"w\",\"why\":\"y\"},{\"severity\":\"should\",\"what\":\"s\"}]}")).unwrap();
        assert_eq!((v, f.len(), f[0].line), (Verdict::Changes, 2, 3));
        assert!(parse(&block("{\"verdict\":\"pass\",\"findings\":[{\"severity\":\"should\",\"what\":\"nit\"}]}")).is_ok());
    }

    #[test]
    fn each_malformed_verdict_says_why() {
        assert!(parse("pass").unwrap_err().contains("no fenced"));
        assert!(parse(&block("{\"verdict\":\"maybe\"}")).unwrap_err().contains("not a verdict"));
        assert!(parse(&block("{\"verdict\":\"changes\",\"findings\":[{\"severity\":\"should\",\"what\":\"x\"}]}")).unwrap_err().contains("at least one finding with severity `must`"));
        assert!(parse(&block("{\"verdict\":\"changes\",\"findings\":[]}")).unwrap_err().contains("at least one"));
        assert!(parse(&block("{\"verdict\":\"pass\",\"findings\":[{\"severity\":\"must\",\"what\":\"x\"}]}")).unwrap_err().contains("cannot have a `must`"));
        assert!(parse(&block("{\"verdict\":\"changes\",\"findings\":[{\"severity\":\"must\",\"what\":\" \"}]}")).unwrap_err().contains("needs `what`"));
    }
}
