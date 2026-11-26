//! The app window's browser in a systemd scope of its own. Under systemd every process
//! of `kagentd.service` is killed when the unit stops, the browser and its helpers
//! included: they crashed and dumped a core at every daemon stop, and the window a
//! next daemon adopts (`AppWindows::adopt_existing`) was gone. In a scope of its own the
//! window outlives a daemon restart; `systemd-run --scope` executes the browser itself,
//! so the child's pid stays the browser's and `close` still ends it.

use std::path::Path;

/// `argv` started in its own transient user scope when the daemon runs under systemd
/// (`under_systemd`: `INVOCATION_ID` is set) and `systemd-run` is at hand; else as it
/// is (pure, tested). The scope's description is fixed: by default systemd takes the
/// command line, which holds the API token in the URL.
pub fn in_own_scope(argv: Vec<String>, under_systemd: bool, systemd_run: Option<&Path>) -> Vec<String> {
    let Some(run) = systemd_run.filter(|_| under_systemd) else { return argv };
    let mut out = vec![run.display().to_string(), "--user".into(), "--scope".into(), "--collect".into(), "--quiet".into(), "--description=KAgent window".into(), "--".into()];
    out.extend(argv);
    out
}

/// Whether this process is a systemd unit's (systemd sets `INVOCATION_ID` for it).
pub fn under_systemd() -> bool {
    std::env::var_os("INVOCATION_ID").is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_systemd_the_browser_gets_a_scope_of_its_own() {
        let argv = vec!["/usr/bin/brave".to_string(), "--app=http://127.0.0.1:7878/?app#token=secret".into()];
        let run = Path::new("/usr/bin/systemd-run");
        let a = in_own_scope(argv.clone(), true, Some(run));
        assert_eq!(a[..7], ["/usr/bin/systemd-run", "--user", "--scope", "--collect", "--quiet", "--description=KAgent window", "--"]);
        assert_eq!(a[7..], argv[..], "the browser's own argv follows unchanged");
        assert!(!a[..7].iter().any(|x| x.contains("token")), "the token is not in the scope's description");
        assert_eq!(in_own_scope(argv.clone(), false, Some(run)), argv, "not under systemd: as it is");
        assert_eq!(in_own_scope(argv.clone(), true, None), argv, "no systemd-run: as it is");
    }
}
