mod types;
mod lifecycle;
mod session;
mod sync;
mod container;
mod render;

use clap::{Parser, Subcommand};
use types::*;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "claude-container", version, about = "Container-isolated Claude Code sessions")]
struct Cli {
    #[arg(short, long)]
    session: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Bidirectional sync
    Sync {
        branch: String,
        #[arg(long)]
        repo: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_verify: bool,
    },
    /// Pull session changes to host
    Pull {
        branch: Option<String>,
        #[arg(long)]
        repo: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_verify: bool,
    },
    /// Push host changes into container
    Push {
        branch: Option<String>,
        #[arg(long)]
        repo: Vec<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Manage session properties
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Check sync status
    Status {
        branch: Option<String>,
        #[arg(long)]
        repo: Option<String>,
    },
    /// Validate an image
    ValidateImage {
        image: String,
    },
}

#[derive(Subcommand)]
enum SessionAction {
    Show,
    SetDir { target: Option<String> },
    Diff { branch: Option<String> },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let session_name = cli.session.map(SessionName::new);

    match cli.command {
        Some(Commands::ValidateImage { image }) => {
            cmd_validate_image(&image).await?;
        }
        Some(Commands::Session { action }) => {
            let name = require_session(session_name)?;
            match action {
                SessionAction::Show => cmd_session_show(&name).await?,
                SessionAction::Diff { branch } => {
                    cmd_sync_preview(&name, &branch.unwrap_or("main".into()), true).await?;
                }
                _ => eprintln!("Not yet implemented"),
            }
        }
        Some(Commands::Sync { branch, dry_run, .. }) => {
            let name = require_session(session_name)?;
            cmd_sync_preview(&name, &branch, dry_run).await?;
        }
        Some(Commands::Status { branch, .. }) => {
            let name = require_session(session_name)?;
            cmd_sync_preview(&name, &branch.unwrap_or("main".into()), true).await?;
        }
        Some(Commands::Pull { branch, dry_run, .. }) => {
            let name = require_session(session_name)?;
            cmd_sync_preview(&name, &branch.unwrap_or("main".into()), dry_run).await?;
        }
        Some(Commands::Push { branch, dry_run, .. }) => {
            let name = require_session(session_name)?;
            cmd_sync_preview(&name, &branch.unwrap_or("main".into()), dry_run).await?;
        }
        None => {
            if let Some(name) = session_name {
                cmd_session_show(&name).await?;
            } else {
                eprintln!("Usage: claude-container -s <session> [command]");
                eprintln!("       claude-container -s <session> sync <branch>");
                eprintln!("       claude-container -s <session> session show");
                eprintln!("       claude-container validate-image <image>");
            }
        }
    }

    Ok(())
}

fn require_session(name: Option<SessionName>) -> anyhow::Result<SessionName> {
    name.ok_or_else(|| anyhow::anyhow!("--session required"))
}

async fn cmd_validate_image(image: &str) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let image_ref = ImageRef::new(image);
    let validation = lc.validate_image(&image_ref).await?;
    render::image_validation(&validation);
    if !validation.is_valid() {
        std::process::exit(1);
    }
    Ok(())
}

async fn cmd_session_show(name: &SessionName) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());
    let discovered = sm.discover(name).await?;
    let config = sm.read_config(name).await.ok().flatten();
    render::session_info(name, &discovered, config.as_ref());
    Ok(())
}

async fn cmd_sync_preview(name: &SessionName, branch: &str, _dry_run: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    // Read config to get repo paths
    let config = sm.read_config(name).await?
        .ok_or_else(|| anyhow::anyhow!("No config in session '{}'", name))?;

    let repo_paths: std::collections::BTreeMap<String, std::path::PathBuf> = config.projects.iter()
        .map(|(pname, pcfg)| (pname.clone(), pcfg.path.clone()))
        .collect();

    let engine = sync::SyncEngine::new(lc.docker_client().clone());
    let plan = engine.plan_sync(name, branch, &repo_paths).await?;

    render::sync_plan(&plan.action);

    Ok(())
}
