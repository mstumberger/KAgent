//! kagent-priv: the only KAgent code that runs as root. Reads one JSON op from stdin,
//! validates it against an allowlist, computes its undo, executes it, prints one JSON
//! response. Spec: docs/specs/priv-ops.md.

pub mod bpf;
pub mod ops;
pub mod pkg;
pub mod system;
