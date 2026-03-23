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
#[command(name = "git-sandbox", version, about = "Container-isolated Claude Code sessions")]
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
            cmd_run(&name, &prompt, dockerfile).await?;
        }
        Commands::Start { session, dockerfile, discover_repos, r#continue, docker, as_root, from_branch, prompt } => {
            let name = SessionName::new(&session);
            cmd_start(&name, dockerfile, discover_repos, r#continue, docker, as_root, from_branch).await?;
        }
        Commands::Session { session, action } => {
            let name = SessionName::new(&session);
            match action.unwrap_or(SessionAction::Show) {
                SessionAction::Show => cmd_session_show(&name).await?,
                SessionAction::Diff { branch } => {
                    cmd_sync_preview(&name, &branch.unwrap_or("main".into())).await?;
                }
                SessionAction::AddRepo { paths } => {
                    cmd_session_add_repo(&name, &paths).await?;
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
    _continue_session: bool,
    enable_docker: bool,
    as_root: bool,
    from_branch: Option<String>,
) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    // Step 1: Discover current session state
    let discovered = sm.discover(name).await?;
    eprintln!("{}", colored::Colorize::blue(format!("→ Session: {}", name).as_str()));

    match &discovered {
        crate::types::DiscoveredSession::DoesNotExist(_) => {
            // Need repos to create a session
            let mut repos = if let Some(ref dir) = discover_repos {
                let found = sm.discover_repos(dir);
                if found.is_empty() {
                    anyhow::bail!("No git repos found in {}", dir.display());
                }
                eprintln!("  Discovered {} repo(s) in {}", found.len(), dir.display());
                found
            } else {
                // Try cwd as a single repo
                let cwd = std::env::current_dir()?;
                if cwd.join(".git").is_dir() {
                    let repo_name = cwd.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or("repo".into());
                    let branch = git2::Repository::open(&cwd)
                        .ok()
                        .and_then(|r| r.head().ok().and_then(|h| h.shorthand().map(|s| s.to_string())));
                    eprintln!("  Using current directory: {}", repo_name);
                    vec![types::RepoConfig {
                        name: repo_name,
                        host_path: cwd,
                        extract_enabled: true,
                        branch,
                    }]
                } else {
                    anyhow::bail!(
                        "No repos to create session. Use --discover-repos <dir> or run from a git repo."
                    );
                }
            };

            // Apply --from-branch override
            if let Some(ref branch) = from_branch {
                for repo in &mut repos {
                    repo.branch = Some(branch.clone());
                }
            }

            // Show repos with branches
            for r in &repos {
                let branch_info = r.branch.as_deref().unwrap_or("HEAD");
                eprintln!("    {} {} ({})", colored::Colorize::blue("·"), r.name, colored::Colorize::dimmed(branch_info));
            }

            // Build config
            let mut projects = std::collections::BTreeMap::new();
            for repo in &repos {
                projects.insert(repo.name.clone(), types::ProjectConfig {
                    path: repo.host_path.clone(),
                    extract: repo.extract_enabled,
                    main: false,
                });
            }
            let config = types::SessionConfig {
                version: Some("1".into()),
                projects,
            };

            // Create volumes
            eprintln!("  Creating volumes...");
            lc.create_volumes(name).await?;

            // TODO: clone repos into session volume
            // For now, save config and let the user know
            sm.save_metadata(&types::SessionMetadata {
                name: name.clone(),
                dockerfile: dockerfile.clone(),
                run_as_rootish: !as_root,
                run_as_user: false,
                enable_docker,
                ephemeral: false,
                no_config: false,
            })?;

            eprintln!("  {} Session '{}' created with {} repo(s)", colored::Colorize::green("✓"), name, repos.len());
            eprintln!("  {} Repo cloning into volume not yet implemented in rust.", colored::Colorize::yellow("⚠"));
            eprintln!("  Use: claude-container -s {} to finish setup via bash.", name);
            return Ok(());
        }
        crate::types::DiscoveredSession::VolumesOnly { .. } => {
            eprintln!("  Session exists, no container.");
        }
        crate::types::DiscoveredSession::Stopped { .. } => {
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
            let token_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".config/claude-container/token");
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

async fn cmd_run(
    name: &SessionName,
    prompt: &str,
    dockerfile: Option<PathBuf>,
) -> anyhow::Result<()> {
    eprintln!("{}", colored::Colorize::blue(format!("→ Running prompt in session '{}'", name).as_str()));
    eprintln!("  Prompt: {}", if prompt.len() > 60 { format!("{}...", &prompt[..60]) } else { prompt.to_string() });

    let lc = lifecycle::Lifecycle::new()?;

    // Step 1: Resolve image
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

    // Step 2: Verified pipeline (same as cmd_start)
    let docker = container::verify_docker(&lc).await?;
    let verified_image = container::verify_image(&lc, &docker, &image).await?;
    for tool in verified_image.validation.missing_optional() {
        eprintln!("  {} {} (optional)", colored::Colorize::yellow("⚠"), tool);
    }
    let volumes = container::verify_volumes(&lc, &docker, name).await?;

    // Token
    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .or_else(|_| {
            let token_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".config/claude-container/token");
            std::fs::read_to_string(&token_file)
        })
        .map_err(|_| anyhow::anyhow!("No auth token found. Set CLAUDE_CODE_OAUTH_TOKEN or create ~/.config/claude-container/token"))?;
    let verified_token = container::verify_token(&lc, token.trim())?;

    // Script dir
    let script_dir = std::env::var("CC_SCRIPT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = dirs::home_dir().unwrap_or_default();
            let candidates = [
                home.join(".local/share/claude-container"),
                home.join("dev/controlflow/juggernautlabs/claude-container"),
            ];
            candidates.into_iter().find(|p| p.join("lib/container/cc-entrypoint").exists())
                .unwrap_or_else(|| PathBuf::from("."))
        });

    // Plan target
    let target = container::plan_target(&lc, &docker, name, &verified_image, &script_dir).await
        .or_else(|e| {
            if let ContainerError::ContainerRunning(ref _ctr) = e {
                // For run mode, we need a fresh container. Remove the running one.
                Err(e)
            } else {
                Err(e)
            }
        })?;

    // Build LaunchReady
    let ready = crate::types::verified::LaunchReady {
        docker,
        image: verified_image,
        volumes,
        token: verified_token,
        container: target,
    };

    // Step 3: Run headless
    eprintln!();
    let output = container::run_headless(&lc, ready, name, &script_dir, prompt).await?;

    // Step 4: Print captured output
    if !output.is_empty() {
        println!("{}", output);
    }

    // Step 5: Try to read .agent-result from the volume
    // (This would require exec-ing into the container or mounting the volume.
    //  For now, just note that the output has been printed.)
    eprintln!();
    eprintln!("{}", colored::Colorize::green("→ Run complete."));

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

async fn cmd_session_add_repo(name: &SessionName, paths: &[PathBuf]) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    // Verify session exists
    let discovered = sm.discover(name).await?;
    match &discovered {
        crate::types::DiscoveredSession::DoesNotExist(_) => {
            anyhow::bail!("Session '{}' does not exist. Use 'start' to create it.", name);
        }
        _ => {}
    }

    let repos_to_add: Vec<_> = if paths.is_empty() {
        // No paths given — use cwd
        let cwd = std::env::current_dir()?;
        if !cwd.join(".git").is_dir() {
            anyhow::bail!("Current directory is not a git repo. Specify paths.");
        }
        let repo_name = cwd.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or("repo".into());
        vec![(repo_name, cwd)]
    } else {
        paths.iter().filter_map(|p| {
            let abs = if p.is_absolute() {
                p.clone()
            } else {
                std::env::current_dir().ok()?.join(p)
            };
            if !abs.join(".git").is_dir() {
                eprintln!("  {} Not a git repo: {}", colored::Colorize::yellow("⚠"), p.display());
                return None;
            }
            let repo_name = abs.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or("repo".into());
            Some((repo_name, abs))
        }).collect()
    };

    if repos_to_add.is_empty() {
        anyhow::bail!("No valid git repos to add.");
    }

    for (repo_name, path) in &repos_to_add {
        eprintln!("  {} {} → {}", colored::Colorize::blue("+"), repo_name, path.display());
    }

    // TODO: actually clone repos into the session volume
    // For now, just report what would be added
    eprintln!();
    eprintln!("  {} {} repo(s) identified", colored::Colorize::green("✓"), repos_to_add.len());
    eprintln!("  {} Cloning into volume not yet implemented in rust.", colored::Colorize::yellow("⚠"));

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
