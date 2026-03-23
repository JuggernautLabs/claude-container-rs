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
        /// Dockerfile or directory containing one
        #[arg(long)]
        dockerfile: Option<PathBuf>,
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
        #[arg(long)]
        repo: Vec<String>,
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
        /// Source branch (default: main)
        branch: Option<String>,
        #[arg(long)]
        repo: Vec<String>,
        #[arg(long)]
        dry_run: bool,
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
        #[arg(long)]
        repo: Vec<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_verify: bool,
    },
    /// Show session info, properties, repos
    Session {
        #[arg(short, long)]
        session: String,
        #[command(subcommand)]
        action: Option<SessionAction>,
    },
    /// Check sync status (read-only)
    Status {
        #[arg(short, long)]
        session: String,
        branch: Option<String>,
        #[arg(long)]
        repo: Option<String>,
    },
    /// Validate a Docker image meets the container protocol
    #[command(name = "validate-image")]
    ValidateImage {
        image: String,
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
    /// Show session info (default)
    Show,
    /// Set startup directory
    SetDir { target: Option<String> },
    /// Show diffs between container and host
    Diff { branch: Option<String> },
    /// Add repos to session
    AddRepo { paths: Vec<PathBuf> },
    /// Clean up stale state
    Cleanup,
    /// Remove container, keep volumes
    Rebuild,
    /// Set a session property
    Set { key: String, value: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::ValidateImage { image } => {
            cmd_validate_image(&image).await?;
        }
        Commands::Run { session, prompt, dockerfile } => {
            let name = SessionName::new(&session);
            eprintln!("→ Running prompt in session '{}'", name);
            eprintln!("  Prompt: {}", if prompt.len() > 60 { format!("{}...", &prompt[..60]) } else { prompt.clone() });
            // TODO: launch with AgentTask::Run { prompt }, capture output
            eprintln!("  ⚠ Run mode not yet implemented");
        }
        Commands::Start { session, dockerfile, discover_repos, r#continue, docker, as_root, prompt } => {
            let name = SessionName::new(&session);
            cmd_start(&name, dockerfile, discover_repos, r#continue, docker, as_root).await?;
        }
        Commands::Session { session, action } => {
            let name = SessionName::new(&session);
            match action.unwrap_or(SessionAction::Show) {
                SessionAction::Show => cmd_session_show(&name).await?,
                SessionAction::Diff { branch } => {
                    cmd_sync_preview(&name, &branch.unwrap_or("main".into())).await?;
                }
                _ => eprintln!("Not yet implemented"),
            }
        }
        Commands::Sync { session, branch, dry_run, .. } => {
            let name = SessionName::new(&session);
            cmd_sync_preview(&name, &branch).await?;
        }
        Commands::Status { session, branch, .. } => {
            let name = SessionName::new(&session);
            cmd_sync_preview(&name, &branch.unwrap_or("main".into())).await?;
        }
        Commands::Pull { session, branch, dry_run, .. } => {
            let name = SessionName::new(&session);
            cmd_sync_preview(&name, &branch.unwrap_or("main".into())).await?;
        }
        Commands::Push { session, branch, dry_run, .. } => {
            let name = SessionName::new(&session);
            cmd_sync_preview(&name, &branch.unwrap_or("main".into())).await?;
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

async fn cmd_start(
    name: &SessionName,
    dockerfile: Option<PathBuf>,
    discover_repos: Option<PathBuf>,
    continue_session: bool,
    enable_docker: bool,
    as_root: bool,
) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    // Step 1: Discover current session state
    let discovered = sm.discover(name).await?;
    eprintln!("{}", colored::Colorize::blue(format!("→ Session: {}", name).as_str()));

    match &discovered {
        crate::types::DiscoveredSession::DoesNotExist(_) => {
            eprintln!("  Creating new session...");
            // TODO: create session (volumes + clone repos)
            // For now, error
            anyhow::bail!("Session creation not yet implemented. Use bash claude-container to create.");
        }
        crate::types::DiscoveredSession::VolumesOnly { .. } => {
            eprintln!("  Session exists, no container.");
        }
        crate::types::DiscoveredSession::Stopped { container, .. } => {
            eprintln!("  Resuming stopped container...");
        }
        crate::types::DiscoveredSession::Running { .. } => {
            eprintln!("  Container already running.");
            eprintln!("  TODO: attach to running container");
            return Ok(());
        }
    }

    // Step 2: Resolve image
    let image = if let Some(ref df) = dockerfile {
        let df_path = if df.is_dir() {
            let candidate = df.join("Dockerfile");
            if candidate.exists() { candidate } else {
                anyhow::bail!("No Dockerfile found in {}", df.display());
            }
        } else {
            df.clone()
        };
        let image_name = format!("claude-dev-{}", name);
        let image_ref = ImageRef::new(&image_name);
        eprintln!("  Building image: {}", image_name);
        lc.build_image(&image_ref, &df_path, &df_path.parent().unwrap_or(&PathBuf::from("."))).await?;
        image_ref
    } else {
        ImageRef::new("ghcr.io/hypermemetic/claude-container:latest")
    };

    // Step 3: Verified pipeline
    let docker = container::verify_docker(&lc).await?;
    let verified_image = container::verify_image(&lc, &docker, &image).await?;
    for tool in verified_image.validation.missing_optional() {
        eprintln!("  {} {} (optional)", colored::Colorize::yellow("⚠"), tool);
    }
    let volumes = container::verify_volumes(&lc, &docker, name).await?;

    // Token — find it from env, file, or keychain
    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .or_else(|_| {
            let token_file = dirs::config_dir()
                .unwrap_or_default()
                .join("claude-container/token");
            std::fs::read_to_string(&token_file)
        })
        .map_err(|_| anyhow::anyhow!("No auth token found. Set CLAUDE_CODE_OAUTH_TOKEN or create ~/.config/claude-container/token"))?;
    let verified_token = container::verify_token(&lc, token.trim())?;

    // Determine entrypoint script dir (where cc-entrypoint lives)
    // For now, use the bash claude-container's lib/container/ dir
    let script_dir = std::env::var("CC_SCRIPT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // Try to find the bash claude-container's lib dir
            let home = dirs::home_dir().unwrap_or_default();
            let candidates = [
                home.join(".local/share/claude-container"),
                home.join("dev/controlflow/juggernautlabs/claude-container"),
            ];
            candidates.into_iter().find(|p| p.join("lib/container/cc-entrypoint").exists())
                .unwrap_or_else(|| PathBuf::from("."))
        });

    // Plan launch target
    let target = container::plan_target(&lc, &docker, name, &verified_image, &script_dir).await
        .or_else(|e| {
            // If container is running, attach instead
            if let ContainerError::ContainerRunning(ref _ctr) = e {
                eprintln!("  Container already running — attaching...");
                // TODO: implement attach to running container
                Err(e)
            } else {
                Err(e)
            }
        })?;

    // Build LaunchReady — all proofs assembled
    let ready = crate::types::verified::LaunchReady {
        docker,
        image: verified_image,
        volumes,
        token: verified_token,
        container: target,
    };

    // Step 4: Launch
    eprintln!();
    container::launch(&lc, ready, name, &script_dir).await?;

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

async fn cmd_sync_preview(name: &SessionName, branch: &str) -> anyhow::Result<()> {
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
