//! The hook helper Claude Code and Codex run on every hook event.
//!
//! It reads one JSON payload from stdin and forwards it to the NiumaTerm
//! instance named by the environment, over the same named pipe the single-
//! instance handoff uses. That transport is the Windows IPC layer, and the
//! agent panes that consume the events are Windows-gated with it, so the
//! binary is inert elsewhere rather than absent: it stays a build target of
//! the workspace on every platform.

#[cfg(windows)]
mod hook;

fn main() {
    #[cfg(windows)]
    hook::run();
}
