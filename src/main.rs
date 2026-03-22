mod types;
mod lifecycle;
mod session;
mod sync;
mod container;

use clap::{Parser, Subcommand};
use types::SessionName;

#[derive(Parser)]
#[command(name = "claude-container", version, about = "Container-isolated Claude Code sessions")]
struct Cli {
    /// Session name
    #[arg(short, long)]
    session: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Pull session changes to host
    Pull {
        /// Target branch to merge into
        branch: Option<String>,
        /// Filter to specific repo(s)
        #[arg(long)]
        repo: Vec<String>,
        /// Preview only
        #[arg(long)]
        dry_run: bool,
        /// Skip confirmation
        #[arg(long)]
        no_verify: bool,
    },
    /// Push host changes into container
    Push {
        /// Source branch
        branch: Option<String>,
        #[arg(long)]
        repo: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, value_enum)]
        strategy: Option<PushStrategy>,
    },
    /// Bidirectional sync
    Sync {
        /// Target branch
        branch: String,
        #[arg(long)]
        repo: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_verify: bool,
    },
    /// Manage session properties
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Manage repos in session
    Repos {
        #[command(subcommand)]
        action: ReposAction,
    },
    /// Check sync status
    Status {
        branch: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        dirty: bool,
    },
    /// Watch for changes and run command
    Watch {
        #[arg(long)]
        repo: Vec<String>,
        /// Command to run on change
        #[arg(last = true)]
        command: Vec<String>,
    },
}

#[derive(clap::ValueEnum, Clone)]
enum PushStrategy {
    Ff,
    Merge,
    Rebase,
}

#[derive(Subcommand)]
enum SessionAction {
    Show,
    SetDir { target: Option<String> },
    Set { key: String, value: String },
    Unset { key: String },
    AddRepo { paths: Vec<String> },
    Diff { branch: Option<String> },
    Cleanup,
    Rebuild,
}

#[derive(Subcommand)]
enum ReposAction {
    List,
    Add { #[arg(long)] repo: Vec<String> },
    Remove { #[arg(long)] repo: Vec<String> },
    Discover,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let session = cli.session.map(SessionName::new);

    match cli.command {
        Some(Commands::Pull { branch, repo, dry_run, no_verify }) => {
            let session = session.ok_or_else(|| anyhow::anyhow!("--session required"))?;
            println!("pull: session={}, branch={:?}", session, branch);
            // TODO: implement
        }
        Some(Commands::Sync { branch, repo, dry_run, no_verify }) => {
            let session = session.ok_or_else(|| anyhow::anyhow!("--session required"))?;
            println!("sync: session={} ↔ {}", session, branch);
        }
        None => {
            // Default: launch session
            if let Some(session) = session {
                println!("launch: session={}", session);
                // TODO: lifecycle flow
            } else {
                eprintln!("Usage: claude-container -s <session> [command]");
            }
        }
        _ => {
            println!("Command not yet implemented");
        }
    }

    Ok(())
}
