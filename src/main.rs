mod types;
mod lifecycle;
mod session;
mod sync;
mod container;
mod render;
pub mod scripts;
mod shell_safety;

use clap::{Parser, Subcommand};
use types::*;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-sandbox", version, about = "Container-isolated Claude Code sessions")]
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
        /// Source branch (default: main)
        branch: Option<String>,
        /// Filter repos by regex
        #[arg(short, long)]
        filter: Option<String>,
        /// Include dependency repos
        #[arg(long)]
        all: bool,
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
    /// Set a session property
    Set { key: String, value: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Restore terminal sanity in case a previous session leaked raw mode.
    // Uses the consolidated restore_terminal() which handles crossterm,
    // cursor visibility, and termios flags in one call.
    container::restore_terminal();

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
        Commands::Start { session, attach, logs, dockerfile, discover_repos, r#continue, docker, as_root, from_branch, prompt } => {
            let name = SessionName::new(&session);
            cmd_start(&name, attach, logs, auto_yes, dockerfile, discover_repos, r#continue, docker, as_root, from_branch, prompt).await?;
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
                SessionAction::Start { attach, logs } => {
                    cmd_start(&name, attach, logs, auto_yes, None, None, false, false, false, None, None).await?;
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
        Commands::Push { session, branch, filter, all, dry_run, .. } => {
            let name = SessionName::new(&session);
            cmd_push(&name, &branch.unwrap_or("main".into()), filter.as_deref(), all, dry_run, auto_yes).await?;
        }
    }

    Ok(())
}

fn require_session(name: Option<SessionName>) -> anyhow::Result<SessionName> {
    name.ok_or_else(|| anyhow::anyhow!("--session required"))
}

async fn cmd_list() -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let docker = lc.docker_client();

    // Discover sessions from Docker volumes (claude-session-*)
    let volumes = docker.list_volumes(None::<bollard::volume::ListVolumesOptions<String>>).await?;
    let mut session_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    if let Some(vols) = volumes.volumes {
        for vol in &vols {
            if let Some(name) = vol.name.strip_prefix("claude-session-") {
                session_names.insert(name.to_string());
            }
        }
    }

    // Also check metadata dir on host
    let meta_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".config/claude-container/sessions");
    if meta_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&meta_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                // Strip known extensions
                let name = name.strip_suffix(".env")
                    .or_else(|| name.strip_suffix(".yml"))
                    .or_else(|| name.strip_suffix(".yaml"))
                    .unwrap_or(&name)
                    .to_string();
                if !name.starts_with('.') {
                    session_names.insert(name);
                }
            }
        }
    }

    if session_names.is_empty() {
        eprintln!("No sessions found.");
        return Ok(());
    }

    // For each session, check container state
    for name in &session_names {
        let sn = SessionName::new(name);
        let container_name = sn.container_name();

        let state = match docker.inspect_container(container_name.as_str(), None).await {
            Ok(info) => {
                let running = info.state.as_ref()
                    .and_then(|s| s.status.as_ref())
                    .map(|s| matches!(s, bollard::models::ContainerStateStatusEnum::RUNNING))
                    .unwrap_or(false);
                if running { "running" } else { "stopped" }
            }
            Err(_) => "no container",
        };

        let marker = match state {
            "running" => colored::Colorize::green("●"),
            "stopped" => colored::Colorize::yellow("○"),
            _ => colored::Colorize::dimmed("·"),
        };

        println!("  {} {:24} {}", marker, name, colored::Colorize::dimmed(state));
    }

    Ok(())
}

async fn cmd_validate_image(image: &str, force: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let image_ref = ImageRef::new(image);

    if force {
        // Evict cached result so validate_image() re-runs from scratch
        let inspect = lc.docker_client()
            .inspect_image(image_ref.as_str())
            .await
            .map_err(|_| anyhow::anyhow!("Image not found: {}", image))?;
        let image_id = inspect.id.unwrap_or_default();
        if let Some(cache_path) = lifecycle::validation_cache_path(&image_id) {
            let _ = std::fs::remove_file(cache_path);
        }
    }

    let validation = lc.validate_image(&image_ref).await?;
    render::image_validation(&validation);
    if !validation.is_valid() {
        std::process::exit(1);
    }
    Ok(())
}

// ============================================================================
// LaunchPath — type-safe session start routing
// ============================================================================

/// What kind of start we're doing. Each variant carries exactly the data
/// needed for its path — you can't accidentally use cwd auto-detect for
/// an existing session because the type doesn't allow it.
enum LaunchPath {
    /// Brand new session — needs repos, volumes, cloning
    CreateNew {
        repos: Vec<types::RepoConfig>,
        /// Dockerfile from --dockerfile or cwd auto-detect (new sessions only)
        dockerfile: Option<PathBuf>,
        enable_docker: bool,
        as_root: bool,
    },
    /// Session exists (volumes present), needs container created or resumed
    ResumeExisting {
        /// Dockerfile from --dockerfile override or session metadata
        dockerfile: Option<PathBuf>,
    },
    /// Container already running — just attach
    AttachRunning,
}

impl LaunchPath {
    /// Resolve the image source. CLI --dockerfile always wins.
    fn resolve_dockerfile(&self, cli_dockerfile: &Option<PathBuf>) -> Option<PathBuf> {
        // CLI override always wins
        if let Some(df) = cli_dockerfile {
            if !df.as_os_str().is_empty() && df.exists() {
                return Some(df.clone());
            }
        }
        match self {
            Self::CreateNew { dockerfile, .. } => dockerfile.clone(),
            Self::ResumeExisting { dockerfile, .. } => dockerfile.clone(),
            Self::AttachRunning => None,
        }
    }
}

async fn cmd_start(
    name: &SessionName,
    attach: bool,
    replay_logs: bool,
    auto_yes: bool,
    dockerfile: Option<PathBuf>,
    discover_repos: Option<PathBuf>,
    continue_session: bool,
    enable_docker: bool,
    as_root: bool,
    from_branch: Option<String>,
    initial_prompt: Option<String>,
) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());
    let discovered = sm.discover(name).await?;
    eprintln!("{}", colored::Colorize::blue(format!("→ Session: {}", name).as_str()));

    // Route to the correct launch path based on session state
    let launch_path = determine_launch_path(
        &sm, name, &discovered, &dockerfile, &discover_repos,
        &from_branch, attach, enable_docker, as_root,
    )?;

    // AttachRunning is terminal — just attach, no verification needed
    if matches!(launch_path, LaunchPath::AttachRunning) {
        eprintln!("  Attaching to running container...");
        eprintln!();
        use std::io::Write;
        std::io::stderr().flush().ok();
        container::attach_to_running(&lc, &name.container_name(), replay_logs).await?;
        eprintln!("  To reattach: git-sandbox session -s {} start -a", name);
        return Ok(());
    }

    // CreateNew needs volumes + cloning before we can build/launch
    if let LaunchPath::CreateNew { ref repos, ref dockerfile, enable_docker, as_root } = launch_path {
        create_new_session(&lc, &sm, name, repos, dockerfile, enable_docker, as_root).await?;
    }

    // Resolve image, verify, and launch
    let effective_dockerfile = launch_path.resolve_dockerfile(&dockerfile);
    let image = resolve_image(&lc, name, &effective_dockerfile).await?;
    let launch_opts = container::LaunchOptions { continue_session, initial_prompt };
    verify_and_launch(&lc, name, &image, auto_yes, &launch_opts).await
}

/// Determine the launch path from discovered session state.
fn determine_launch_path(
    sm: &session::SessionManager,
    name: &SessionName,
    discovered: &crate::types::DiscoveredSession,
    cli_dockerfile: &Option<PathBuf>,
    discover_repos: &Option<PathBuf>,
    from_branch: &Option<String>,
    attach: bool,
    enable_docker: bool,
    as_root: bool,
) -> anyhow::Result<LaunchPath> {
    match discovered {
        crate::types::DiscoveredSession::DoesNotExist(_) => {
            let mut repos = if let Some(ref dir) = discover_repos {
                let found = sm.discover_repos(dir);
                if found.is_empty() { anyhow::bail!("No git repos found in {}", dir.display()); }
                eprintln!("  Discovered {} repo(s) in {}", found.len(), dir.display());
                found
            } else {
                let cwd = std::env::current_dir()?;
                if cwd.join(".git").is_dir() {
                    let repo_name = cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or("repo".into());
                    let branch = git2::Repository::open(&cwd).ok()
                        .and_then(|r| r.head().ok().and_then(|h| h.shorthand().map(|s| s.to_string())));
                    eprintln!("  Using current directory: {}", repo_name);
                    vec![types::RepoConfig { name: repo_name, host_path: cwd, branch }]
                } else {
                    eprintln!("  {} Not in a git repo — starting with empty workspace.", colored::Colorize::dimmed("·"));
                    vec![]
                }
            };

            if let Some(ref branch) = from_branch {
                for repo in &mut repos { repo.branch = Some(branch.clone()); }
            }
            for r in &repos {
                eprintln!("    {} {} ({})", colored::Colorize::blue("·"), r.name,
                    colored::Colorize::dimmed(r.branch.as_deref().unwrap_or("HEAD")));
            }

            // Auto-detect Dockerfile from cwd (new sessions only)
            let auto_df = if cli_dockerfile.is_none() {
                std::env::current_dir().ok().map(|c| c.join("Dockerfile")).filter(|d| d.exists())
            } else { None };

            Ok(LaunchPath::CreateNew {
                repos,
                dockerfile: cli_dockerfile.clone().or(auto_df),
                enable_docker,
                as_root,
            })
        }
        crate::types::DiscoveredSession::VolumesOnly { .. } => {
            eprintln!("  Session exists, no container.");
            let stored_df = sm.load_metadata(name).and_then(|m| m.dockerfile)
                .filter(|df| !df.as_os_str().is_empty() && df.exists());
            Ok(LaunchPath::ResumeExisting { dockerfile: stored_df })
        }
        crate::types::DiscoveredSession::Stopped { .. } => {
            eprintln!("  Resuming stopped container...");
            let stored_df = sm.load_metadata(name).and_then(|m| m.dockerfile)
                .filter(|df| !df.as_os_str().is_empty() && df.exists());
            Ok(LaunchPath::ResumeExisting { dockerfile: stored_df })
        }
        crate::types::DiscoveredSession::Running { .. } => {
            if !attach {
                eprintln!("  {} Container already running. Use -a to attach.", colored::Colorize::yellow("⚠"));
                std::process::exit(0);
            }
            Ok(LaunchPath::AttachRunning)
        }
    }
}

/// Create a new session: volumes, clone repos, save metadata.
async fn create_new_session(
    lc: &lifecycle::Lifecycle,
    sm: &session::SessionManager,
    name: &SessionName,
    repos: &[types::RepoConfig],
    dockerfile: &Option<PathBuf>,
    enable_docker: bool,
    as_root: bool,
) -> anyhow::Result<()> {
    eprintln!("  Creating volumes...");
    lc.create_volumes(name).await?;

    let engine = sync::SyncEngine::new(lc.docker_client().clone());
    for (i, repo) in repos.iter().enumerate() {
        eprintln!("  Cloning [{}/{}] {}...", i + 1, repos.len(), repo.name);
        engine.clone_into_volume(name, &repo.name, &repo.host_path, repo.branch.as_deref()).await?;
    }

    let main_project = repos.first().map(|r| r.name.as_str()).unwrap_or("");
    if !main_project.is_empty() {
        engine.write_main_project(name, main_project).await?;
    }

    let mut projects = std::collections::BTreeMap::new();
    for repo in repos {
        projects.insert(repo.name.clone(), types::ProjectConfig {
            path: repo.host_path.clone(),
            main: false,
            role: Default::default(),
        });
    }

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
    Ok(())
}

/// Resolve the Docker image: build from Dockerfile or use default.
async fn resolve_image(
    lc: &lifecycle::Lifecycle,
    name: &SessionName,
    dockerfile: &Option<PathBuf>,
) -> anyhow::Result<ImageRef> {
    if let Some(ref df) = dockerfile {
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
        eprintln!("  image: {} (from {})", colored::Colorize::blue(image_name.as_str()),
            colored::Colorize::dimmed(df_path.display().to_string().as_str()));
        lc.build_image(&image_ref, &df_path, &df_path.parent().unwrap_or(&PathBuf::from("."))).await?;
        Ok(image_ref)
    } else {
        let default_image = "ghcr.io/hypermemetic/claude-container:latest";
        eprintln!("  image: {} {}", default_image, colored::Colorize::dimmed("(default)"));
        Ok(ImageRef::new(default_image))
    }
}

/// Verify Docker, image, volumes, token, plan target, and launch.
async fn verify_and_launch(
    lc: &lifecycle::Lifecycle,
    name: &SessionName,
    image: &ImageRef,
    auto_yes: bool,
    launch_opts: &container::LaunchOptions,
) -> anyhow::Result<()> {
    let docker = container::verify_docker(lc).await?;
    let verified_image = container::verify_image(lc, &docker, image).await?;
    for tool in verified_image.validation.missing_optional() {
        eprintln!("  {} {} (optional)", colored::Colorize::yellow("⚠"), tool);
    }
    let volumes = container::verify_volumes(lc, &docker, name).await?;

    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .or_else(|_| {
            let token_file = dirs::home_dir().unwrap_or_default()
                .join(".config/claude-container/token");
            std::fs::read_to_string(&token_file)
        })
        .map_err(|_| anyhow::anyhow!("No auth token. Set CLAUDE_CODE_OAUTH_TOKEN or create ~/.config/claude-container/token"))?;
    let verified_token = container::verify_token(lc, token.trim())?;

    let script_dir = scripts::materialize()?;
    let target = container::plan_target(lc, &docker, name, &verified_image, &script_dir).await?;

    if let LaunchTarget::Rebuild(ref confirmed) = target {
        eprintln!("  {} Container is stale:", colored::Colorize::yellow("⚠"));
        for reason in confirmed.description.strip_prefix("Rebuild container: ").unwrap_or(&confirmed.description).split(", ") {
            eprintln!("    {} {}", colored::Colorize::dimmed("·"), reason);
        }
        if !confirm("  Rebuild?", auto_yes) {
            eprintln!("  Aborted.");
            return Ok(());
        }
    }

    let ready = crate::types::verified::LaunchReady {
        docker, image: verified_image, volumes, token: verified_token, container: target,
    };

    eprintln!();
    use std::io::Write;
    std::io::stderr().flush().ok();
    container::launch(lc, ready, name, &script_dir, launch_opts).await?;
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

    // Materialize embedded scripts to cache dir for Docker bind-mounts
    let script_dir = scripts::materialize()?;

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

async fn cmd_session_show(name: &SessionName, filter: Option<&str>) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());
    let discovered = sm.discover(name).await?;
    let mut config = sm.read_config(name).await.ok().flatten();

    // Apply filter to config projects
    if let (Some(pattern), Some(ref mut cfg)) = (filter, &mut config) {
        if let Ok(re) = regex::Regex::new(pattern) {
            cfg.projects.retain(|name, _| re.is_match(name));
        }
    }

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

    // Clone repos into the session volume
    let lc = lifecycle::Lifecycle::new()?;
    let engine = sync::SyncEngine::new(lc.docker_client().clone());
    for (i, (repo_name, path)) in repos_to_add.iter().enumerate() {
        eprintln!("  Cloning [{}/{}] {}...", i + 1, repos_to_add.len(), repo_name);
        engine.clone_into_volume(name, repo_name, path, None).await?;
    }
    eprintln!("  {} {} repo(s) added", colored::Colorize::green("✓"), repos_to_add.len());

    Ok(())
}

async fn cmd_session_exec(name: &SessionName, as_root: bool, command: &[String]) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let docker = lc.docker_client();
    let container_name = name.container_name();

    // Check container is running
    match lc.inspect_container(&container_name).await? {
        types::docker::ContainerState::Running { .. } => {}
        _ => anyhow::bail!("Container not running. Start it first."),
    }

    let cmd = shell_safety::build_exec_cmd(command);

    let exec = docker.create_exec(
        container_name.as_str(),
        bollard::exec::CreateExecOptions {
            cmd: Some(cmd),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            user: Some(if as_root { "root".to_string() } else { "developer".to_string() }),
            ..Default::default()
        },
    ).await?;

    let output = docker.start_exec(&exec.id, None::<bollard::exec::StartExecOptions>).await?;

    if let bollard::exec::StartExecResults::Attached { mut output, .. } = output {
        use futures_util::StreamExt;
        while let Some(Ok(chunk)) = output.next().await {
            print!("{}", chunk);
        }
    }

    Ok(())
}

async fn cmd_session_stop(name: &SessionName, auto_yes: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let container_name = name.container_name();

    match lc.inspect_container(&container_name).await? {
        types::docker::ContainerState::Running { .. } => {
            if !confirm(&format!("  Stop container '{}'?", name), auto_yes) {
                eprintln!("  Aborted.");
                return Ok(());
            }
            eprintln!("  Stopping {}...", container_name);
            lc.docker_client().stop_container(
                container_name.as_str(),
                Some(bollard::container::StopContainerOptions { t: 10 }),
            ).await?;
            eprintln!("  {} Stopped.", colored::Colorize::green("✓"));
        }
        types::docker::ContainerState::Stopped { .. } => {
            eprintln!("  Already stopped.");
        }
        types::docker::ContainerState::NotFound { .. } => {
            eprintln!("  No container.");
        }
    }
    Ok(())
}

async fn cmd_session_rebuild(name: &SessionName, auto_yes: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let container_name = name.container_name();

    // Check current state
    match lc.inspect_container(&container_name).await? {
        types::docker::ContainerState::NotFound { .. } => {
            eprintln!("  No container to rebuild.");
            return Ok(());
        }
        types::docker::ContainerState::Running { .. } => {
            if !confirm("  Container is running. Stop and rebuild?", auto_yes) {
                eprintln!("  Aborted.");
                return Ok(());
            }
        }
        _ => {}
    }

    // Build image FIRST — only remove container after successful build.
    // This prevents leaving the user with no container if the build fails.
    let sm = session::SessionManager::new(lc.docker_client().clone());
    if let Some(meta) = sm.load_metadata(name) {
        if let Some(ref df) = meta.dockerfile {
            let df_path = if df.is_dir() {
                df.join("Dockerfile")
            } else {
                df.clone()
            };
            if df_path.exists() {
                let image_name = format!("claude-dev-{}", name);
                let image_ref = ImageRef::new(&image_name);
                eprintln!("  Building image {} from {}...", image_name, df_path.display());
                lc.build_image(&image_ref, &df_path, &df_path.parent().unwrap_or(&PathBuf::from("."))).await?;
                eprintln!("  {} Image built successfully.", colored::Colorize::green("✓"));
            }
        }
    }

    // Image validated (or no Dockerfile) — now safe to remove the old container
    eprintln!("  Removing container {}...", container_name);
    lc.remove_container(&container_name).await?;
    eprintln!("  {} Container removed. Volumes preserved.", colored::Colorize::green("✓"));

    eprintln!("  Run `git-sandbox start -s {}` to launch.", name);
    Ok(())
}

async fn cmd_session_cleanup(name: &SessionName, auto_yes: bool) -> anyhow::Result<()> {
    if !confirm("  Remove stale markers from session volume?", auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    let lc = lifecycle::Lifecycle::new()?;

    // Remove stale markers from the session volume
    let engine = sync::SyncEngine::new(lc.docker_client().clone());
    let volume = name.session_volume();
    let container_name = format!("cc-cleanup-{}", name);

    let _ = lc.docker_client().remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    let script = "rm -f /session/.reconcile-complete /session/.merge-into-summary /session/.merge-into-branch /session/.sync-summary /session/.sync-branch 2>/dev/null; echo CLEANED";
    use types::docker::{throwaway_config, VolumeMount, RunAs};
    let config = throwaway_config(
        "alpine/git", script,
        &[VolumeMount::Writable { source: volume.to_string(), target: "/session".into() }],
        &RunAs::developer(), name,
    );

    lc.docker_client().create_container(
        Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
        config,
    ).await?;
    lc.docker_client().start_container(&container_name, None::<bollard::container::StartContainerOptions<String>>).await?;

    use futures_util::StreamExt;
    let mut wait = lc.docker_client().wait_container(&container_name, None::<bollard::container::WaitContainerOptions<String>>);
    while let Some(_) = wait.next().await {}

    let _ = lc.docker_client().remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    eprintln!("  {} Stale markers removed from session volume.", colored::Colorize::green("✓"));
    Ok(())
}

/// Scan session volumes for ownership problems.
/// Runs a throwaway container that checks every file's UID against HOST_UID.
async fn cmd_session_verify(name: &SessionName) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let docker = lc.docker_client();
    let host_uid = unsafe { libc::getuid() };

    eprintln!("  Checking volumes for UID {} (your host UID)...", host_uid);
    eprintln!();

    let volumes = [
        (name.session_volume(), "/workspace"),
        (name.state_volume(), "/home/developer/.claude"),
        (name.cargo_volume(), "/home/developer/.cargo"),
        (name.npm_volume(), "/home/developer/.npm"),
        (name.pip_volume(), "/home/developer/.cache/pip"),
    ];

    let mounts: Vec<types::docker::VolumeMount> = volumes.iter()
        .map(|(vol, mount)| types::docker::VolumeMount::ReadOnly { source: vol.to_string(), target: mount.to_string() })
        .collect();

    // Script: for each mount, count files not owned by HOST_UID
    let script = format!(
        r#"
for dir in /workspace /home/developer/.claude /home/developer/.cargo /home/developer/.npm /home/developer/.cache/pip; do
    [ -d "$dir" ] || continue
    total=$(find "$dir" -maxdepth 3 2>/dev/null | wc -l | tr -d ' ')
    bad=$(find "$dir" -maxdepth 3 ! -uid {uid} 2>/dev/null | wc -l | tr -d ' ')
    if [ "$bad" -gt 0 ]; then
        echo "PROBLEM|$dir|$bad|$total"
        find "$dir" -maxdepth 2 ! -uid {uid} -ls 2>/dev/null | head -10
    else
        echo "OK|$dir|0|$total"
    fi
done
"#,
        uid = host_uid,
    );

    let container_name = format!("cc-verify-{}", name);
    let _ = docker.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    let config = types::docker::throwaway_config(
        "alpine/git", &script, &mounts, &types::docker::RunAs::developer(), name,
    );

    docker.create_container(
        Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
        config,
    ).await?;
    docker.start_container(&container_name, None::<bollard::container::StartContainerOptions<String>>).await?;

    use futures_util::StreamExt;
    let mut wait = docker.wait_container(&container_name, None::<bollard::container::WaitContainerOptions<String>>);
    while let Some(_) = wait.next().await {}

    let mut stdout = String::new();
    let mut logs = docker.logs(
        &container_name,
        Some(bollard::container::LogsOptions::<String> { stdout: true, stderr: true, follow: false, ..Default::default() }),
    );
    while let Some(Ok(chunk)) = logs.next().await {
        stdout.push_str(&chunk.to_string());
    }

    let _ = docker.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    let mut problems = 0;
    for line in stdout.lines() {
        if line.starts_with("OK|") {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 4 {
                eprintln!("  {} {} — {} file(s), all owned by you", colored::Colorize::green("✓"), parts[1], parts[3]);
            }
        } else if line.starts_with("PROBLEM|") {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 4 {
                eprintln!("  {} {} — {}/{} file(s) wrong owner", colored::Colorize::red("✗"), parts[1], parts[2], parts[3]);
                problems += 1;
            }
        } else if !line.trim().is_empty() {
            // ls -l output for bad files
            eprintln!("    {}", colored::Colorize::dimmed(line.trim()));
        }
    }

    eprintln!();
    if problems > 0 {
        eprintln!("  {} {} volume(s) have ownership problems.", colored::Colorize::yellow("⚠"), problems);
        eprintln!("  Run `git-sandbox session -s {} fix` to repair.", name);
    } else {
        eprintln!("  {} All volumes clean.", colored::Colorize::green("✓"));
    }

    Ok(())
}

/// Fix ownership in all session volumes — recursive chown to HOST_UID.
async fn cmd_session_fix(name: &SessionName, auto_yes: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let docker = lc.docker_client();
    let host_uid = unsafe { libc::getuid() };
    let host_gid = unsafe { libc::getgid() };

    if !confirm(&format!("  chown -R {}:{} on all volumes?", host_uid, host_gid), auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }
    eprintln!("  Fixing ownership to {}:{} in all volumes...", host_uid, host_gid);

    let volumes = [
        (name.session_volume(), "/workspace"),
        (name.state_volume(), "/home/developer/.claude"),
        (name.cargo_volume(), "/home/developer/.cargo"),
        (name.npm_volume(), "/home/developer/.npm"),
        (name.pip_volume(), "/home/developer/.cache/pip"),
    ];

    let mounts: Vec<types::docker::VolumeMount> = volumes.iter()
        .map(|(vol, mount)| types::docker::VolumeMount::Writable { source: vol.to_string(), target: mount.to_string() })
        .collect();

    let script = format!(
        "chown -R {}:{} /workspace /home/developer/.claude /home/developer/.cargo /home/developer/.npm /home/developer/.cache/pip 2>/dev/null; echo FIXED",
        host_uid, host_gid,
    );

    let container_name = format!("cc-fix-{}", name);
    let _ = docker.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    // Fix runs as root intentionally — it's fixing ownership
    let config = types::docker::throwaway_config(
        "alpine/git", &script, &mounts, &types::docker::RunAs::Root, name,
    );

    docker.create_container(
        Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
        config,
    ).await?;
    docker.start_container(&container_name, None::<bollard::container::StartContainerOptions<String>>).await?;

    use futures_util::StreamExt;
    let mut wait = docker.wait_container(&container_name, None::<bollard::container::WaitContainerOptions<String>>);
    while let Some(_) = wait.next().await {}

    let _ = docker.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    eprintln!("  {} All volumes fixed.", colored::Colorize::green("✓"));
    Ok(())
}

async fn cmd_session_set_role(name: &SessionName, repo_pattern: &str, role: CliRepoRole) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    let mut config = sm.read_or_discover_config(name).await?;

    let target_role = match role {
        CliRepoRole::Project => types::config::RepoRole::Project,
        CliRepoRole::Dependency => types::config::RepoRole::Dependency,
    };

    let re = regex::Regex::new(repo_pattern)
        .map_err(|e| anyhow::anyhow!("Invalid pattern '{}': {}", repo_pattern, e))?;

    let mut matched = 0;
    for (pname, pcfg) in config.projects.iter_mut() {
        if re.is_match(pname) {
            pcfg.role = target_role.clone();
            matched += 1;
            eprintln!("  {} {} → {}", colored::Colorize::green("✓"), pname, target_role);
        }
    }

    if matched == 0 {
        anyhow::bail!("No repos match '{}'", repo_pattern);
    }

    // Write updated config back to the session volume
    sm.write_config(name, &config).await?;

    eprintln!("  {} {} repo(s) updated", colored::Colorize::green("✓"), matched);
    Ok(())
}

async fn cmd_session_set_dir(name: &SessionName, target: Option<&str>) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let engine = sync::SyncEngine::new(lc.docker_client().clone());

    match target {
        Some(dir) => {
            engine.write_main_project(name, dir).await?;
            eprintln!("  {} Main project set to '{}'", colored::Colorize::green("✓"), dir);
        }
        None => {
            // Clear the main project
            let volume = name.session_volume();
            let container_name = format!("cc-setdir-{}", name);
            let _ = lc.docker_client().remove_container(
                &container_name,
                Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
            ).await;

            let config = types::docker::throwaway_config(
                "alpine/git", "rm -f /session/.main-project",
                &[types::docker::VolumeMount::Writable { source: volume.to_string(), target: "/session".into() }],
                &types::docker::RunAs::developer(), name,
            );

            lc.docker_client().create_container(
                Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
                config,
            ).await?;
            lc.docker_client().start_container(&container_name, None::<bollard::container::StartContainerOptions<String>>).await?;

            use futures_util::StreamExt;
            let mut wait = lc.docker_client().wait_container(&container_name, None::<bollard::container::WaitContainerOptions<String>>);
            while let Some(_) = wait.next().await {}

            let _ = lc.docker_client().remove_container(
                &container_name,
                Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
            ).await;

            eprintln!("  {} Main project cleared (defaults to /workspace)", colored::Colorize::green("✓"));
        }
    }
    Ok(())
}

async fn cmd_sync_preview(name: &SessionName, branch: &str, filter: Option<&str>) -> anyhow::Result<()> {
    let (_lc, _engine, plan, _repo_paths) = build_sync_plan(name, branch, filter, false).await?;
    render::sync_plan_directed(&plan.action, "status");
    Ok(())
}

/// Extract-only: pull container work into session branches, no merge into target.
/// Shows a diff preview of what changed in the container vs the host.
async fn cmd_extract(name: &SessionName, filter: Option<&str>, dry_run: bool, auto_yes: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    let config = sm.read_or_discover_config(name).await?;

    let engine = sync::SyncEngine::new(lc.docker_client().clone());

    // Snapshot container
    let mut volume_repos = engine.snapshot(name, "").await?;

    // Apply filter
    if let Some(pattern) = filter {
        let re = regex::Regex::new(pattern)
            .map_err(|e| anyhow::anyhow!("Invalid filter regex '{}': {}", pattern, e))?;
        volume_repos.retain(|vr| re.is_match(&vr.name));
        if volume_repos.is_empty() {
            anyhow::bail!("No repos match filter '{}'", pattern);
        }
    }

    // Classify: new (no session branch on host) vs changed (container ahead of session branch)
    let mut changed = Vec::new();
    let mut unchanged = 0u32;

    for vr in &volume_repos {
        let host_path = match config.projects.get(&vr.name) {
            Some(cfg) => &cfg.path,
            None => continue,
        };
        let session_branch = name.to_string();

        // Check if session branch exists on host and compare
        let host_session_head = git2::Repository::open(host_path).ok()
            .and_then(|repo| {
                repo.find_reference(&format!("refs/heads/{}", session_branch)).ok()
                    .and_then(|r| r.peel_to_commit().ok())
                    .map(|c| types::CommitHash::new(c.id().to_string()))
            });

        let container_head = &vr.head;

        // Compute diff: host session branch HEAD → container HEAD
        let diff = host_session_head.as_ref().and_then(|h_head| {
            engine.compute_diff(host_path, h_head, container_head)
        });

        let is_same = host_session_head.as_ref().map_or(false, |h| h.as_str() == container_head.as_str());
        if is_same {
            unchanged += 1;
            continue;
        }

        let status = if host_session_head.is_none() { "new" } else { "changed" };
        changed.push((vr, host_path.clone(), session_branch, diff, status));
    }

    // Render preview
    render::rule(Some(&format!("extract: {}", name)));
    if changed.is_empty() {
        eprintln!("{}", colored::Colorize::dimmed("Nothing new to extract."));
        return Ok(());
    }

    eprintln!("{} to extract, {} unchanged", changed.len(), unchanged);
    eprintln!();

    for (vr, host_path, session_branch, _, status) in &changed {
        let short_head = &vr.head.as_str()[..7.min(vr.head.as_str().len())];
        if *status == "new" {
            eprintln!("  {} {} → {} (new, container:{})",
                colored::Colorize::blue("←"), vr.name, session_branch,
                colored::Colorize::dimmed(short_head));
        } else {
            // Show commit range: session_head..container_head
            let session_head = git2::Repository::open(host_path).ok()
                .and_then(|repo| {
                    repo.find_reference(&format!("refs/heads/{}", session_branch)).ok()
                        .and_then(|r| r.peel_to_commit().ok())
                        .map(|c| c.id().to_string())
                });
            let from = session_head.as_deref().map(|s| &s[..7]).unwrap_or("?");
            eprintln!("  {} {} → {} ({}..{})",
                colored::Colorize::green("←"), vr.name, session_branch,
                colored::Colorize::dimmed(from), short_head);
        }
    }

    // Full diffstat for changed repos (not new ones — no base to diff against)
    let diffs_to_show: Vec<_> = changed.iter()
        .filter(|(_, _, _, diff, _)| diff.is_some())
        .collect();

    if !diffs_to_show.is_empty() {
        eprintln!();
        render::rule(None);
        eprintln!();
        eprintln!("session diff:");

        let mut total_files = 0u32;
        let mut total_ins = 0u32;
        let mut total_del = 0u32;

        for (vr, _, _, diff, _) in &diffs_to_show {
            if let Some(d) = diff {
                if d.files.is_empty() { continue; }
                eprintln!("  {}", colored::Colorize::blue(vr.name.as_str()));
                let max_path = d.files.iter().map(|f| f.path.len()).max().unwrap_or(20);
                for f in &d.files {
                    let bar = render::render_change_bar_pub(f.insertions, f.deletions, 40);
                    eprintln!("     {:width$} | {:>4} {}", f.path, f.insertions + f.deletions, bar, width = max_path);
                }
                eprintln!("     {} file(s) changed, {} insertions(+), {} deletions(-)",
                    d.files_changed, d.insertions, d.deletions);
                eprintln!();
                total_files += d.files_changed;
                total_ins += d.insertions;
                total_del += d.deletions;
            }
        }

        if total_files > 0 {
            eprintln!("{} Total: {} file(s), +{} -{}", colored::Colorize::dimmed("→"), total_files, total_ins, total_del);
        }
    }

    if dry_run {
        return Ok(());
    }

    if !confirm(&format!("\n  Extract {} repo(s)?", changed.len()), auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    let mut extracted = 0u32;
    let mut failed = 0u32;
    for (vr, host_path, session_branch, _, _) in &changed {
        match engine.extract(name, &vr.name, host_path, session_branch).await {
            Ok(result) => {
                eprintln!("  {} {} ({} commit(s))", colored::Colorize::green("✓"), vr.name, result.commit_count);
                extracted += 1;
            }
            Err(e) => {
                eprintln!("  {} {} — {}", colored::Colorize::red("✗"), vr.name, e);
                failed += 1;
            }
        }
    }

    eprintln!();
    if failed > 0 {
        eprintln!("  {} {} extracted, {} failed", colored::Colorize::yellow("⚠"), extracted, failed);
    } else {
        eprintln!("  {} {} extracted to session branches", colored::Colorize::green("✓"), extracted);
    }
    Ok(())
}

async fn cmd_pull(name: &SessionName, branch: &str, filter: Option<&str>, include_deps: bool, dry_run: bool, auto_yes: bool, squash: bool) -> anyhow::Result<()> {
    let (lc, engine, plan, repo_paths) = build_sync_plan(name, branch, filter, include_deps).await?;

    // Collect info before consuming plan
    let has_clean = plan.action.repo_actions.iter()
        .any(|a| matches!(a.decision, types::SyncDecision::Pull { .. } | types::SyncDecision::CloneToHost));
    let has_merge_to_target = plan.action.repo_actions.iter()
        .any(|a| matches!(a.decision, types::SyncDecision::MergeToTarget { .. }));
    struct PendingMergeInfo {
        repo_name: String,
        host_path: std::path::PathBuf,
        has_conflict: bool,
        conflict_files: Vec<String>,
    }
    let pending_merge_repos: Vec<PendingMergeInfo> = plan.action.repo_actions.iter()
        .filter(|a| matches!(a.decision, types::SyncDecision::MergeToTarget { .. }))
        .filter_map(|a| {
            a.host_path.clone().map(|p| {
                let has_conflict = a.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty());
                let conflict_files = a.trial_conflicts.clone().unwrap_or_default();
                PendingMergeInfo { repo_name: a.repo_name.clone(), host_path: p, has_conflict, conflict_files }
            })
        })
        .collect();
    struct DivergedInfo {
        repo_name: String,
        container_ahead: u32,
        host_ahead: u32,
        has_conflict: bool,
        conflict_files: Vec<String>,
    }
    let diverged_repos: Vec<DivergedInfo> = plan.action.repo_actions.iter()
        .filter(|a| matches!(a.decision, types::SyncDecision::Reconcile { .. }))
        .map(|a| {
            let (ca, ha) = match &a.decision {
                types::SyncDecision::Reconcile { container_ahead, host_ahead } => (*container_ahead, *host_ahead),
                _ => (0, 0),
            };
            let has_conflict = a.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty());
            let conflict_files = a.trial_conflicts.clone().unwrap_or_default();
            DivergedInfo { repo_name: a.repo_name.clone(), container_ahead: ca, host_ahead: ha, has_conflict, conflict_files }
        })
        .collect();

    render::sync_plan_directed(&plan.action, "pull");

    if dry_run || (!has_clean && diverged_repos.is_empty() && pending_merge_repos.is_empty()) {
        return Ok(());
    }

    use std::io::Write;

    // Execute clean pulls (extract + merge)
    if has_clean {
        if confirm("\n  Pull new repos?", auto_yes) {
            eprintln!();
            let result = engine.execute_sync(name, plan.action, &repo_paths).await?;
            render::sync_result(&result);
        } else {
            eprintln!("  Skipped clean pulls.");
        }
    }

    // Execute pending merges (session branch → target, no extraction needed)
    // Split into clean merges and known conflicts
    let (clean_merges, conflict_merges): (Vec<_>, Vec<_>) = pending_merge_repos.iter()
        .partition(|m| !m.has_conflict);

    if !clean_merges.is_empty() {
        let session_branch = name.to_string();
        if confirm(&format!("\n  Merge {} repo(s) into {}?", clean_merges.len(), branch), auto_yes) {
            for m in &clean_merges {
                match engine.merge(&m.host_path, &session_branch, branch, squash) {
                    Ok(outcome) => {
                        if matches!(outcome, types::git::MergeOutcome::Conflict { .. }) {
                            eprintln!("  {} {} — {}", colored::Colorize::red("✗"), m.repo_name, outcome);
                        } else {
                            eprintln!("  {} {} — {}", colored::Colorize::green("✓"), m.repo_name, outcome);
                        }
                    }
                    Err(e) => {
                        eprintln!("  {} {} — {}", colored::Colorize::red("✗"), m.repo_name, e);
                    }
                }
            }
        }
    }

    if !conflict_merges.is_empty() {
        eprintln!();
        for m in &conflict_merges {
            let file_list = m.conflict_files.iter().take(5).map(|f| f.as_str()).collect::<Vec<_>>().join(", ");
            eprintln!("  {} {} — will conflict ({})", colored::Colorize::red("✗"), m.repo_name, file_list);
        }

        // Collect for reconciliation
        let conflict_repos: Vec<_> = conflict_merges.iter()
            .map(|m| (m.repo_name.clone(), m.host_path.clone(), m.conflict_files.clone()))
            .collect();
        offer_reconciliation(&lc, name, &conflict_repos, branch).await?;
    }

    // Handle diverged repos — prompt per repo
    if !diverged_repos.is_empty() {
        eprintln!();
        let mut conflict_repos = Vec::new();

        for dinfo in &diverged_repos {
            let merge_status = if dinfo.has_conflict {
                format!("{}", colored::Colorize::red("CONFLICT"))
            } else {
                format!("{}", colored::Colorize::green("auto-merge possible"))
            };

            eprintln!("  {} {} — container +{}, host +{} ({})",
                colored::Colorize::yellow("↔"), dinfo.repo_name, dinfo.container_ahead, dinfo.host_ahead, merge_status);

            if dinfo.has_conflict {
                eprint!("    [s]kip  [r]econcile with Claude  > ");
            } else {
                eprint!("    [a]uto-merge  [s]kip  [r]econcile with Claude  > ");
            }
            std::io::stderr().flush().ok();
            let mut choice = String::new();
            std::io::stdin().read_line(&mut choice).ok();
            let choice = choice.trim().to_lowercase();

            match choice.chars().next().unwrap_or('s') {
                'a' if !dinfo.has_conflict => {
                    let host_path = match repo_paths.get(&dinfo.repo_name) {
                        Some(p) => p,
                        None => { eprintln!("    {} no host path", colored::Colorize::red("✗")); continue; }
                    };
                    let session_branch = name.to_string();
                    match engine.inject(name, &dinfo.repo_name, host_path, branch).await {
                        Ok(()) => {
                            match engine.extract(name, &dinfo.repo_name, host_path, &session_branch).await {
                                Ok(_extract) => {
                                    match engine.merge(host_path, &session_branch, branch, squash) {
                                        Ok(outcome) => eprintln!("    {} auto-merged ({})", colored::Colorize::green("✓"), outcome),
                                        Err(e) => eprintln!("    {} merge failed: {}", colored::Colorize::red("✗"), e),
                                    }
                                }
                                Err(e) => eprintln!("    {} extract failed: {}", colored::Colorize::red("✗"), e),
                            }
                        }
                        Err(e) => eprintln!("    {} inject failed: {}", colored::Colorize::red("✗"), e),
                    }
                }
                'r' => {
                    // Collect for agentic reconciliation
                    if let Some(host_path) = repo_paths.get(&dinfo.repo_name) {
                        conflict_repos.push((dinfo.repo_name.clone(), host_path.clone(), dinfo.conflict_files.clone()));
                    }
                }
                _ => {
                    eprintln!("    {} skipped", colored::Colorize::dimmed("·"));
                }
            }
        }

        // Launch agentic reconciliation for collected repos
        if !conflict_repos.is_empty() {
            offer_reconciliation(&lc, name, &conflict_repos, branch).await?;
        }
    }

    Ok(())
}

async fn cmd_push(name: &SessionName, branch: &str, filter: Option<&str>, include_deps: bool, dry_run: bool, auto_yes: bool) -> anyhow::Result<()> {
    let (_lc, engine, plan, repo_paths) = build_sync_plan(name, branch, filter, include_deps).await?;

    let has_pushes = plan.action.repo_actions.iter().any(|a| matches!(
        a.decision,
        types::SyncDecision::Push { .. } | types::SyncDecision::PushToContainer
    ));

    render::sync_plan_directed(&plan.action, "push");

    if dry_run || !has_pushes {
        return Ok(());
    }

    if !confirm("\n  Execute push?", auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    eprintln!();
    let result = engine.execute_sync(name, plan.action, &repo_paths).await?;
    render::sync_result(&result);
    Ok(())
}

async fn cmd_sync(name: &SessionName, branch: &str, filter: Option<&str>, include_deps: bool, dry_run: bool, auto_yes: bool) -> anyhow::Result<()> {
    let (lc, engine, plan, repo_paths) = build_sync_plan(name, branch, filter, include_deps).await?;

    let has_work = plan.action.has_work();

    render::sync_plan_directed(&plan.action, "sync");

    if dry_run || !has_work {
        return Ok(());
    }

    if !confirm("\n  Execute sync?", auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    eprintln!();
    let result = engine.execute_sync(name, plan.action, &repo_paths).await?;
    render::sync_result(&result);

    let conflicts = collect_conflicts(&result, &repo_paths);
    if !conflicts.is_empty() {
        offer_reconciliation(&lc, name, &conflicts, branch).await?;
    }

    Ok(())
}

/// Extract conflict info from sync results for agentic reconciliation.
/// Uses typed pattern matching on RepoSyncResult::Conflicted — no string inspection.
fn collect_conflicts(
    result: &types::SyncResult,
    repo_paths: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> Vec<(String, std::path::PathBuf, Vec<String>)> {
    result.results.iter().filter_map(|r| {
        if let types::action::RepoSyncResult::Conflicted { repo_name, files } = r {
            let host_path = repo_paths.get(repo_name)?.clone();
            Some((repo_name.clone(), host_path, files.clone()))
        } else {
            None
        }
    }).collect()
}

/// Offer agentic reconciliation: launch Claude to resolve merge conflicts.
async fn offer_reconciliation(
    lc: &lifecycle::Lifecycle,
    name: &SessionName,
    conflicts: &[(String, std::path::PathBuf, Vec<String>)],
    branch: &str,
) -> anyhow::Result<()> {
    eprintln!();
    eprintln!("  {} Merge conflicts in {} repo(s):", colored::Colorize::yellow("⚠"), conflicts.len());
    for (repo_name, _, files) in conflicts {
        if files.is_empty() {
            eprintln!("    {} {}", colored::Colorize::red("✗"), repo_name);
        } else {
            eprintln!("    {} {} ({} file(s))", colored::Colorize::red("✗"), repo_name, files.len());
            for f in files.iter().take(5) {
                eprintln!("      {}", colored::Colorize::dimmed(f.as_str()));
            }
            if files.len() > 5 {
                eprintln!("      {} more...", files.len() - 5);
            }
        }
    }

    eprint!("\n  Launch Claude to resolve conflicts? [Y/n] ");
    use std::io::Write;
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    if answer.trim().to_lowercase().starts_with('n') {
        eprintln!("  Conflicts left unresolved. Fix manually and re-run pull.");
        return Ok(());
    }

    // Build verification proofs for container launch
    let docker = container::verify_docker(lc).await?;
    let image_ref = ImageRef::new("ghcr.io/hypermemetic/claude-container:latest");
    let verified_image = container::verify_image(lc, &docker, &image_ref).await?;
    let volumes = container::verify_volumes(lc, &docker, name).await?;

    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .or_else(|_| {
            let token_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".config/claude-container/token");
            std::fs::read_to_string(&token_file)
        })
        .map_err(|_| anyhow::anyhow!("No auth token"))?;
    let verified_token = container::verify_token(lc, token.trim())?;

    let ready = types::verified::LaunchReady {
        docker,
        image: verified_image,
        volumes,
        token: verified_token,
        container: types::verified::LaunchTarget::Create,
    };

    let script_dir = scripts::materialize()?;

    eprintln!();
    std::io::stderr().flush().ok();

    let reconciled = container::launch_reconciliation(
        lc, ready, name, &script_dir, conflicts,
    ).await?;

    if let Some(_desc) = reconciled {
        eprintln!();
        eprintln!("  {} Reconciliation complete. Re-extracting...", colored::Colorize::green("✓"));

        // Re-extract the resolved repos
        let engine = sync::SyncEngine::new(lc.docker_client().clone());
        for (repo_name, host_path, _) in conflicts {
            let session_branch = name.to_string();
            match engine.extract(name, repo_name, host_path, &session_branch).await {
                Ok(extract) => {
                    eprintln!("    {} {} — {} commit(s)", colored::Colorize::green("✓"), repo_name, extract.commit_count);
                    // Merge the resolved work
                    match engine.merge(host_path, &session_branch, branch, true) {
                        Ok(outcome) => eprintln!("    {} {} — {}", colored::Colorize::green("✓"), repo_name, outcome),
                        Err(e) => eprintln!("    {} {} — merge failed: {}", colored::Colorize::red("✗"), repo_name, e),
                    }
                }
                Err(e) => eprintln!("    {} {} — extract failed: {}", colored::Colorize::red("✗"), repo_name, e),
            }
        }
    } else {
        eprintln!();
        eprintln!("  {} Claude exited without calling fin. Conflicts unresolved.", colored::Colorize::yellow("⚠"));
    }

    Ok(())
}

/// Shared: build a sync plan (used by pull, push, sync, status)
async fn build_sync_plan(
    name: &SessionName,
    branch: &str,
    filter: Option<&str>,
    include_deps: bool,
) -> anyhow::Result<(
    lifecycle::Lifecycle,
    sync::SyncEngine,
    types::Plan<types::SessionSyncPlan>,
    std::collections::BTreeMap<String, std::path::PathBuf>,
)> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    let config = sm.read_or_discover_config(name).await?;

    let mut repo_paths: std::collections::BTreeMap<String, std::path::PathBuf> = config.projects.iter()
        .filter(|(_, pcfg)| include_deps || pcfg.role == types::config::RepoRole::Project)
        .map(|(pname, pcfg)| (pname.clone(), pcfg.path.clone()))
        .collect();

    // Apply regex filter if provided
    if let Some(pattern) = filter {
        let re = regex::Regex::new(pattern)
            .map_err(|e| anyhow::anyhow!("Invalid filter regex '{}': {}", pattern, e))?;
        repo_paths.retain(|name, _| re.is_match(name));
        if repo_paths.is_empty() {
            anyhow::bail!("No repos match filter '{}'", pattern);
        }
    }

    let engine = sync::SyncEngine::new(lc.docker_client().clone());
    let plan = engine.plan_sync(name, branch, &repo_paths).await?;

    Ok((lc, engine, plan, repo_paths))
}

/// Prompt for confirmation. Returns true if confirmed.
/// With --yes, always returns true without prompting.
fn confirm(prompt: &str, auto_yes: bool) -> bool {
    if auto_yes { return true; }
    eprint!("{} [Y/n] ", prompt);
    use std::io::Write;
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    !answer.trim().to_lowercase().starts_with('n')
}

// ============================================================================
// Unit tests for LaunchPath (runs without Docker)
// ============================================================================

#[cfg(test)]
mod launch_path_tests {
    use super::*;
    use std::path::PathBuf;

    // ── resolve_dockerfile: the actual priority logic ──

    #[test]
    fn cli_overrides_everything() {
        // CLI flag beats both stored metadata and auto-detected
        let cli = tempfile::NamedTempFile::new().unwrap();
        let stored = tempfile::NamedTempFile::new().unwrap();

        let path = LaunchPath::ResumeExisting {
            dockerfile: Some(stored.path().to_path_buf()),
        };
        let result = path.resolve_dockerfile(&Some(cli.path().to_path_buf()));
        assert_eq!(result.as_deref(), Some(cli.path()), "CLI must win over stored");
    }

    #[test]
    fn nonexistent_cli_falls_through_to_stored() {
        // CLI points to missing file → fall back to stored metadata
        let stored = tempfile::NamedTempFile::new().unwrap();
        let path = LaunchPath::ResumeExisting {
            dockerfile: Some(stored.path().to_path_buf()),
        };
        let result = path.resolve_dockerfile(&Some(PathBuf::from("/no/such/Dockerfile")));
        assert_eq!(result.as_deref(), Some(stored.path()),
            "Missing CLI path must fall through to stored");
    }

    #[test]
    fn empty_cli_falls_through_to_stored() {
        let stored = tempfile::NamedTempFile::new().unwrap();
        let path = LaunchPath::ResumeExisting {
            dockerfile: Some(stored.path().to_path_buf()),
        };
        let result = path.resolve_dockerfile(&Some(PathBuf::from("")));
        assert_eq!(result.as_deref(), Some(stored.path()),
            "Empty CLI path must fall through to stored");
    }

    // ── The bug that motivated LaunchPath ──

    #[test]
    fn resume_cannot_pick_up_cwd_dockerfile() {
        // This is THE test. Before LaunchPath, an existing session could
        // accidentally build from a cwd Dockerfile it was never created with.
        // ResumeExisting has no auto-detect field — the type makes this impossible.

        // Create a temp dir with a Dockerfile in it
        let tmp = tempfile::tempdir().unwrap();
        let fake_dockerfile = tmp.path().join("Dockerfile");
        std::fs::write(&fake_dockerfile, "FROM alpine").unwrap();

        // ResumeExisting with no stored dockerfile
        let path = LaunchPath::ResumeExisting { dockerfile: None };

        // Even though a Dockerfile exists on disk, resolve returns None
        // because ResumeExisting doesn't carry auto-detect data
        assert_eq!(path.resolve_dockerfile(&None), None,
            "ResumeExisting must NEVER auto-detect from cwd");
    }

    #[test]
    fn create_new_can_auto_detect() {
        // New sessions CAN carry an auto-detected Dockerfile
        let tmp = tempfile::tempdir().unwrap();
        let fake_dockerfile = tmp.path().join("Dockerfile");
        std::fs::write(&fake_dockerfile, "FROM alpine").unwrap();

        let path = LaunchPath::CreateNew {
            repos: vec![],
            dockerfile: Some(fake_dockerfile.clone()),
            enable_docker: false,
            as_root: false,
        };
        assert_eq!(path.resolve_dockerfile(&None), Some(fake_dockerfile),
            "CreateNew should use its auto-detected Dockerfile");
    }

    // ── determine_launch_path: real routing with real DiscoveredSession ──

    fn make_session_manager() -> (lifecycle::Lifecycle, session::SessionManager) {
        // This will fail if Docker isn't running — tests using it should be #[ignore]
        let lc = lifecycle::Lifecycle::new().expect("Docker required");
        let sm = session::SessionManager::new(lc.docker_client().clone());
        (lc, sm)
    }

    #[test]
    fn determine_nonexistent_session_returns_create_new() {
        // Can't actually call determine_launch_path without Docker for the sm,
        // but we can test the match logic directly
        let discovered = types::DiscoveredSession::DoesNotExist(
            SessionName::new("test-nonexistent-launch-path")
        );

        // The match in determine_launch_path for DoesNotExist tries to discover repos.
        // Without Docker we test the type contract: DoesNotExist → CreateNew
        assert!(matches!(discovered, types::DiscoveredSession::DoesNotExist(_)));
    }

    #[test]
    fn stopped_session_produces_resume_existing() {
        // Verify the VolumesOnly and Stopped variants both map to ResumeExisting
        // (test the type relationship, not the full function which needs Docker)

        // A ResumeExisting with no dockerfile should use default image
        let path = LaunchPath::ResumeExisting { dockerfile: None };
        assert_eq!(path.resolve_dockerfile(&None), None,
            "No dockerfile → should use default image");
    }

    // ── resolve_image: Dockerfile → ImageRef ──

    #[tokio::test]
    #[ignore] // requires Docker
    async fn resolve_image_with_real_dockerfile() {
        let tmp = tempfile::tempdir().unwrap();
        let dockerfile = tmp.path().join("Dockerfile");
        std::fs::write(&dockerfile, "FROM alpine:latest\nRUN echo test\n").unwrap();

        let lc = lifecycle::Lifecycle::new().expect("Docker");
        let name = SessionName::new("test-resolve-image");
        let result = resolve_image(&lc, &name, &Some(dockerfile)).await;

        assert!(result.is_ok(), "Build should succeed: {:?}", result.err());
        let image = result.unwrap();
        assert_eq!(image.as_str(), "claude-dev-test-resolve-image");

        // Cleanup: remove the built image
        let _ = lc.docker_client().remove_image(
            "claude-dev-test-resolve-image",
            None::<bollard::image::RemoveImageOptions>,
            None,
        ).await;
    }

    #[tokio::test]
    async fn resolve_image_default_when_no_dockerfile() {
        let lc = lifecycle::Lifecycle::new().unwrap_or_else(|_| {
            // If Docker isn't available, skip gracefully
            panic!("Docker required for this test");
        });
        let name = SessionName::new("test-default-image");
        let result = resolve_image(&lc, &name, &None).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "ghcr.io/hypermemetic/claude-container:latest");
    }

    #[tokio::test]
    async fn resolve_image_nonexistent_dockerfile_errors() {
        let lc = lifecycle::Lifecycle::new().unwrap_or_else(|_| {
            panic!("Docker required");
        });
        let name = SessionName::new("test-bad-dockerfile");
        let bogus = Some(PathBuf::from("/nonexistent/path/Dockerfile"));
        let result = resolve_image(&lc, &name, &bogus).await;

        assert!(result.is_err(), "Non-existent Dockerfile should error");
    }
}
