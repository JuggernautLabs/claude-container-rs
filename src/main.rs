mod types;
mod lifecycle;
mod session;
mod sync;
mod container;
mod render;
pub mod scripts;
mod shell_safety;
mod watch;
mod cmd;

use clap::{Parser, Subcommand};
use types::*;
use std::path::PathBuf;

use cmd::*;

#[derive(Parser)]
#[command(name = "gitvm", version, about = "Container-isolated Claude Code sessions")]
struct Cli {
    /// Skip all confirmation prompts (use in scripts)
    #[arg(short = 'y', long, global = true)]
    yes: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a Claude Code session in a container
    Start {
        /// Session name
        #[arg(short, long)]
        session: String,
        /// Attach to an already-running container
        #[arg(short, long)]
        attach: bool,
        /// Replay container output history when attaching
        #[arg(short, long)]
        logs: bool,
        /// Dockerfile or directory containing one
        #[arg(long)]
        dockerfile: Option<PathBuf>,
        /// Use a pre-built image (skip Dockerfile build)
        #[arg(long, conflicts_with = "dockerfile")]
        image: Option<String>,
        /// Discover repos in directory
        #[arg(long)]
        discover_repos: Option<PathBuf>,
        /// Continue previous Claude conversation
        #[arg(long, short)]
        r#continue: bool,
        /// Enable Docker-in-Docker
        #[arg(long)]
        docker: bool,
        /// Run as root (no privilege drop)
        #[arg(long)]
        as_root: bool,
        /// Clone from this branch instead of each repo's current branch
        #[arg(long)]
        from_branch: Option<String>,
        /// Initial prompt for Claude (interactive)
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Run a prompt in a session and exit (non-interactive)
    Run {
        #[arg(short, long)]
        session: String,
        /// The prompt to execute
        prompt: String,
        /// Dockerfile or directory containing one
        #[arg(long)]
        dockerfile: Option<PathBuf>,
    },
    /// Extract container changes to host session branch, merge into target
    Pull {
        #[arg(short, long)]
        session: String,
        /// Target branch to merge into (omit for extract-only)
        branch: Option<String>,
        /// Filter repos by regex (e.g. "plexus-" or "synapse|gamma")
        #[arg(short, long)]
        filter: Option<String>,
        /// Include dependency repos (default: project repos only)
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_verify: bool,
        #[arg(long)]
        squash: Option<bool>,
    },
    /// Inject host branch into container
    Push {
        #[arg(short, long)]
        session: String,
        /// Source branch (default: session name)
        branch: Option<String>,
        /// Filter repos by regex
        #[arg(short, long)]
        filter: Option<String>,
        /// Include dependency repos
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        /// Force: reset container to match host branch (discards container changes)
        #[arg(long)]
        force: bool,
        /// Merge strategy
        #[arg(long, value_enum)]
        strategy: Option<PushStrategy>,
    },
    /// Bidirectional sync — extract, inject, or reconcile per repo
    Sync {
        #[arg(short, long)]
        session: String,
        /// Target branch
        branch: String,
        /// Filter repos by regex
        #[arg(short, long)]
        filter: Option<String>,
        /// Include dependency repos
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_verify: bool,
    },
    /// Show session info, properties, repos
    Session {
        #[arg(short, long)]
        session: String,
        /// Filter repos by regex
        #[arg(short, long)]
        filter: Option<String>,
        #[command(subcommand)]
        action: Option<SessionAction>,
    },
    /// Check sync status (read-only)
    Status {
        #[arg(short, long)]
        session: String,
        branch: Option<String>,
        /// Filter repos by regex
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// List all sessions
    #[command(name = "ls")]
    List,
    /// Validate a Docker image meets the container protocol
    #[command(name = "validate-image")]
    ValidateImage {
        image: String,
        /// Force revalidation, ignoring any cached result
        #[arg(long)]
        force: bool,
    },
}

#[derive(clap::ValueEnum, Clone)]
enum CliRepoRole {
    Project,
    Dependency,
}

#[derive(clap::ValueEnum, Clone)]
enum PushStrategy {
    Ff,
    Merge,
    Rebase,
}

#[derive(Subcommand)]
enum SessionAction {
    /// Show session info (default)
    Show,
    /// Set startup directory
    SetDir { target: Option<String> },
    /// Show diffs between container and host
    Diff { branch: Option<String> },
    /// Add repos to session
    AddRepo { paths: Vec<PathBuf> },
    /// Start (or resume) the session container
    Start {
        /// Attach to already-running container
        #[arg(short, long)]
        attach: bool,
        /// Replay logs when attaching
        #[arg(short, long)]
        logs: bool,
        /// Dockerfile or directory containing one
        #[arg(long)]
        dockerfile: Option<PathBuf>,
        /// Use a pre-built image (skip Dockerfile build)
        #[arg(long, conflicts_with = "dockerfile")]
        image: Option<String>,
        /// Discover repos in directory
        #[arg(long)]
        discover_repos: Option<PathBuf>,
        /// Continue previous Claude conversation
        #[arg(long, short)]
        r#continue: bool,
        /// Enable Docker-in-Docker
        #[arg(long)]
        docker: bool,
        /// Run as root (no privilege drop)
        #[arg(long)]
        as_root: bool,
        /// Clone from this branch instead of each repo's current branch
        #[arg(long)]
        from_branch: Option<String>,
        /// Initial prompt for Claude
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Stop a running container
    Stop,
    /// Clean up stale state
    Cleanup,
    /// Remove container, keep volumes
    Rebuild,
    /// Run a command inside the running container
    Exec {
        /// Run as root instead of developer
        #[arg(long)]
        root: bool,
        /// Command to run
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Check volumes for ownership/permission problems
    Verify,
    /// Fix ownership problems in volumes
    Fix,
    /// Set repo role (project or dependency)
    SetRole {
        /// Repo name or regex
        repo: String,
        /// Role: project or dependency
        #[arg(value_enum)]
        role: CliRepoRole,
    },
    /// Watch for changes in container and host repos
    Watch {
        /// Poll interval in seconds
        #[arg(long, default_value = "3")]
        interval: u64,
        /// Command to run on change (after --)
        #[arg(trailing_var_arg = true, last = true)]
        command: Vec<String>,
    },
    /// Set a session property
    Set { key: String, value: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Restore terminal sanity in case a previous session leaked raw mode.
    container::restore_terminal();

    // Global Ctrl-C handler — ensures process exits even during blocking Docker calls.
    // The attach loop installs its own handler that overrides this one.
    let _ = ctrlc::set_handler(move || {
        container::restore_terminal();
        eprintln!("\nInterrupted.");
        std::process::exit(130);
    });

    let cli = Cli::parse();
    let auto_yes = cli.yes;

    match cli.command {
        Commands::List => {
            cmd_list().await?;
        }
        Commands::ValidateImage { image, force } => {
            cmd_validate_image(&image, force).await?;
        }
        Commands::Run { session, prompt, dockerfile } => {
            let name = SessionName::new(&session);
            cmd_run(&name, &prompt, dockerfile).await?;
        }
        Commands::Start { session, attach, logs, dockerfile, image, discover_repos, r#continue, docker, as_root, from_branch, prompt } => {
            let name = SessionName::new(&session);
            cmd_start(&name, attach, logs, auto_yes, dockerfile, image, discover_repos, r#continue, docker, as_root, from_branch, prompt).await?;
        }
        Commands::Session { session, filter, action } => {
            let name = SessionName::new(&session);
            let f = filter.as_deref();
            match action.unwrap_or(SessionAction::Show) {
                SessionAction::Show => cmd_session_show(&name, f).await?,
                SessionAction::Diff { branch } => {
                    cmd_sync_preview(&name, &branch.unwrap_or("main".into()), f).await?;
                }
                SessionAction::AddRepo { paths } => {
                    cmd_session_add_repo(&name, &paths).await?;
                }
                SessionAction::Exec { root, command } => {
                    cmd_session_exec(&name, root, &command).await?;
                }
                SessionAction::Start { attach, logs, dockerfile, image, discover_repos, r#continue, docker, as_root, from_branch, prompt } => {
                    cmd_start(&name, attach, logs, auto_yes, dockerfile, image, discover_repos, r#continue, docker, as_root, from_branch, prompt).await?;
                }
                SessionAction::Stop => {
                    cmd_session_stop(&name, auto_yes).await?;
                }
                SessionAction::Rebuild => {
                    cmd_session_rebuild(&name, auto_yes).await?;
                }
                SessionAction::Cleanup => {
                    cmd_session_cleanup(&name, auto_yes).await?;
                }
                SessionAction::Verify => {
                    cmd_session_verify(&name).await?;
                }
                SessionAction::Fix => {
                    cmd_session_fix(&name, auto_yes).await?;
                }
                SessionAction::SetDir { target } => {
                    cmd_session_set_dir(&name, target.as_deref()).await?;
                }
                SessionAction::SetRole { repo, role } => {
                    cmd_session_set_role(&name, &repo, role).await?;
                }
                SessionAction::Watch { interval, command } => {
                    cmd_watch(&name, f, interval, &command).await?;
                }
                SessionAction::Set { key, value } => {
                    eprintln!("Setting {}={} — not yet wired to metadata", key, value);
                }
            }
        }
        Commands::Sync { session, branch, filter, all, dry_run, .. } => {
            let name = SessionName::new(&session);
            cmd_sync(&name, &branch, filter.as_deref(), all, dry_run, auto_yes).await?;
        }
        Commands::Status { session, branch, filter, .. } => {
            let name = SessionName::new(&session);
            cmd_sync_preview(&name, &branch.unwrap_or("main".into()), filter.as_deref()).await?;
        }
        Commands::Pull { session, branch, filter, all, dry_run, squash, .. } => {
            let name = SessionName::new(&session);
            let use_squash = squash.unwrap_or(true);
            if let Some(branch) = branch {
                cmd_pull(&name, &branch, filter.as_deref(), all, dry_run, auto_yes, use_squash).await?;
            } else {
                cmd_extract(&name, filter.as_deref(), dry_run, auto_yes).await?;
            }
        }
        Commands::Push { session, branch, filter, all, dry_run, force, .. } => {
            let name = SessionName::new(&session);
            cmd_push(&name, &branch.unwrap_or(session.clone()), filter.as_deref(), all, dry_run, auto_yes, force).await?;
        }
    }

    Ok(())
}
