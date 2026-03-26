pub(crate) mod start;
pub(crate) mod session;
pub(crate) mod pull;
pub(crate) mod push;
pub(crate) mod sync_cmd;
pub(crate) mod watch;
pub(crate) mod list;
pub(crate) mod run;
pub(crate) mod validate;

// Re-export all command functions for main.rs dispatch
pub(crate) use start::cmd_start;
pub(crate) use session::{
    cmd_session_show, cmd_session_add_repo, cmd_session_exec,
    cmd_session_stop, cmd_session_rebuild, cmd_session_cleanup,
    cmd_session_verify, cmd_session_fix, cmd_session_set_role,
    cmd_session_set_dir,
};
pub(crate) use pull::{cmd_pull, cmd_extract, collect_conflicts, offer_reconciliation};
pub(crate) use push::cmd_push;
pub(crate) use sync_cmd::{cmd_sync, cmd_sync_preview, build_sync_plan};
pub(crate) use watch::cmd_watch;
pub(crate) use list::cmd_list;
pub(crate) use run::cmd_run;
pub(crate) use validate::cmd_validate_image;

// Shared types used across command modules
pub(crate) use super::PushStrategy;
pub(crate) use super::CliRepoRole;

/// Prompt for confirmation. Returns true if confirmed.
/// With --yes, always returns true without prompting.
pub(crate) fn confirm(prompt: &str, auto_yes: bool) -> bool {
    if auto_yes { return true; }
    eprint!("{} [Y/n] ", prompt);
    use std::io::Write;
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    !answer.trim().to_lowercase().starts_with('n')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_auto_yes_returns_true() {
        assert!(confirm("test prompt", true));
    }

    #[test]
    fn confirm_auto_yes_ignores_prompt_content() {
        assert!(confirm("", true));
        assert!(confirm("dangerous operation", true));
    }
}
