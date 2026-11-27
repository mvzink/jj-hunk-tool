//! Hunk-level operations for Jujutsu (jj).
//!
//! The crate is a library as well as the `jj-hunk-tool` binary so that the
//! pieces a test can usefully drive in-process — diff decoding, hunk
//! identification and the coverage markers — are reachable from
//! `tests/*.rs` without spawning a subprocess.

pub mod diff;
pub mod markers;
pub mod tool;
