//! Fixed console-subsystem supervisor for the Windows GUI host.
fn main() {
    if !matches!(
        std::env::args().nth(1).as_deref(),
        Some("--terminal-shell" | "--harness-stdio")
    ) {
        std::process::exit(125);
    }
    std::process::exit(agent_workspace_backend::terminal_shell::run());
}
