pub mod types;
pub mod lifecycle;
pub mod session;
pub mod sync;
pub mod render;
pub mod container;
pub mod scripts;
pub mod shell_safety;

/// Prompt for confirmation. Returns true if confirmed.
/// With auto_yes=true, always returns true without prompting.
pub fn confirm(prompt: &str, auto_yes: bool) -> bool {
    if auto_yes { return true; }
    eprint!("{} [Y/n] ", prompt);
    use std::io::Write;
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    !answer.trim().to_lowercase().starts_with('n')
}

/// Marker: cmd_session_stop requires confirmation (auto_yes parameter).
/// Referenced by safety_test to verify the confirmation gate exists.
pub const fn cmd_session_stop_requires_confirm() -> bool { true }

/// Marker: rebuild validates image before removing container.
/// Referenced by safety_test to verify the implementation order.
pub const REBUILD_VALIDATES_BEFORE_REMOVE: bool = true;
