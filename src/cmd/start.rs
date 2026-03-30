use crate::types::*;
use crate::lifecycle;
use crate::session;
use crate::sync;
use crate::container;
use crate::scripts;
use std::path::PathBuf;
use colored::Colorize;

use super::confirm;

// ============================================================================
// LaunchPath — type-safe session start routing
// ============================================================================

/// What kind of start we're doing. Each variant carries exactly the data
/// needed for its path — you can't accidentally use cwd auto-detect for
/// an existing session because the type doesn't allow it.
pub(crate) enum LaunchPath {
    /// Brand new session — needs repos, volumes, cloning
    CreateNew {
        repos: Vec<RepoConfig>,
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
    pub(crate) fn resolve_dockerfile(&self, cli_dockerfile: &Option<PathBuf>) -> Option<PathBuf> {
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

pub(crate) async fn cmd_start(
    name: &SessionName,
    attach: bool,
    replay_logs: bool,
    auto_yes: bool,
    dockerfile: Option<PathBuf>,
    cli_image: Option<String>,
    discover_repos: Option<PathBuf>,
    continue_session: bool,
    enable_docker: bool,
    as_root: bool,
    from_branch: Option<String>,
    initial_prompt: Option<String>,
) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    lc.ensure_util_image().await;
    let sm = session::SessionManager::new(lc.docker_client().clone());
    let discovered = sm.discover(name).await?;
    eprintln!("{}", format!("→ Session: {}", name).as_str().blue());

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
        eprintln!("  To reattach: gitvm session -s {} start -a", name);
        return Ok(());
    }

    // CreateNew needs volumes + cloning before we can build/launch
    if let LaunchPath::CreateNew { ref repos, ref dockerfile, enable_docker, as_root } = launch_path {
        create_new_session(&lc, &sm, name, repos, dockerfile, enable_docker, as_root).await?;
    }

    // Resolve image: --image wins, then --dockerfile, then stored/default
    let image = if let Some(ref img) = cli_image {
        eprintln!("  image: {} {}", img.as_str().blue(), "(pre-built)".dimmed());
        ImageRef::new(img)
    } else {
        let effective_dockerfile = launch_path.resolve_dockerfile(&dockerfile);
        resolve_image(&lc, name, &effective_dockerfile).await?
    };
    let launch_opts = container::LaunchOptions { continue_session, initial_prompt };
    verify_and_launch(&lc, name, &image, auto_yes, &launch_opts).await
}

/// Determine the launch path from discovered session state.
pub(crate) fn determine_launch_path(
    sm: &session::SessionManager,
    name: &SessionName,
    discovered: &DiscoveredSession,
    cli_dockerfile: &Option<PathBuf>,
    discover_repos: &Option<PathBuf>,
    from_branch: &Option<String>,
    attach: bool,
    enable_docker: bool,
    as_root: bool,
) -> anyhow::Result<LaunchPath> {
    match discovered {
        DiscoveredSession::DoesNotExist(_) => {
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
                    vec![RepoConfig { name: repo_name, host_path: cwd, branch }]
                } else {
                    eprintln!("  {} Not in a git repo — starting with empty workspace.", "·".dimmed());
                    vec![]
                }
            };

            if let Some(ref branch) = from_branch {
                for repo in &mut repos { repo.branch = Some(branch.clone()); }
            }
            for r in &repos {
                eprintln!("    {} {} ({})", "·".blue(), r.name,
                    r.branch.as_deref().unwrap_or("HEAD").dimmed());
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
        DiscoveredSession::VolumesOnly { .. } => {
            eprintln!("  Session exists, no container.");
            let stored_df = sm.load_metadata(name).and_then(|m| m.dockerfile)
                .filter(|df| !df.as_os_str().is_empty() && df.exists());
            Ok(LaunchPath::ResumeExisting { dockerfile: stored_df })
        }
        DiscoveredSession::Stopped { .. } => {
            eprintln!("  Resuming stopped container...");
            let stored_df = sm.load_metadata(name).and_then(|m| m.dockerfile)
                .filter(|df| !df.as_os_str().is_empty() && df.exists());
            Ok(LaunchPath::ResumeExisting { dockerfile: stored_df })
        }
        DiscoveredSession::Running { .. } => {
            if !attach {
                eprintln!("  {} Container already running. Use -a to attach.", "⚠".yellow());
                std::process::exit(0);
            }
            Ok(LaunchPath::AttachRunning)
        }
    }
}

/// Create a new session: volumes, clone repos, save metadata.
pub(crate) async fn create_new_session(
    lc: &lifecycle::Lifecycle,
    sm: &session::SessionManager,
    name: &SessionName,
    repos: &[RepoConfig],
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
        projects.insert(repo.name.clone(), ProjectConfig {
            path: repo.host_path.clone(),
            main: false,
            role: Default::default(),
        });
    }

    sm.save_metadata(&SessionMetadata {
        name: name.clone(),
        dockerfile: dockerfile.clone(),
        run_as_rootish: !as_root,
        run_as_user: false,
        enable_docker,
        ephemeral: false,
        no_config: false,
    })?;

    eprintln!("  {} Session '{}' created with {} repo(s)", "✓".green(), name, repos.len());
    Ok(())
}

/// Resolve the Docker image: build from Dockerfile or use default.
pub(crate) async fn resolve_image(
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
        eprintln!("  image: {} (from {})", image_name.as_str().blue(),
            df_path.display().to_string().as_str().dimmed());
        // Use a minimal temp dir as build context — avoids sending GB of build artifacts.
        // The Dockerfile shouldn't COPY source; workspace is mounted as a volume.
        let build_context = tempfile::tempdir()?;
        std::fs::copy(&df_path, build_context.path().join("Dockerfile"))?;
        lc.build_image(&image_ref, &build_context.path().join("Dockerfile"), build_context.path()).await?;
        Ok(image_ref)
    } else {
        let default_image = "ghcr.io/hypermemetic/claude-container:latest";
        eprintln!("  image: {} {}", default_image, "(default)".dimmed());
        Ok(ImageRef::new(default_image))
    }
}

/// Verify Docker, image, volumes, token, plan target, and launch.
pub(crate) async fn verify_and_launch(
    lc: &lifecycle::Lifecycle,
    name: &SessionName,
    image: &ImageRef,
    auto_yes: bool,
    launch_opts: &container::LaunchOptions,
) -> anyhow::Result<()> {
    let docker = container::verify_docker(lc).await?;
    let verified_image = container::verify_image(lc, &docker, image).await?;
    for tool in verified_image.validation.missing_optional() {
        eprintln!("  {} {} (optional)", "⚠".yellow(), tool);
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
        eprintln!("  {} Container is stale:", "⚠".yellow());
        for reason in confirmed.description.strip_prefix("Rebuild container: ").unwrap_or(&confirmed.description).split(", ") {
            eprintln!("    {} {}", "·".dimmed(), reason);
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
    let launch_result = container::launch(lc, ready, name, &script_dir, launch_opts).await;
    container::restore_terminal(); // safety: always restore even on error
    launch_result?;
    Ok(())
}

// ============================================================================
// Unit tests for LaunchPath (runs without Docker)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ── resolve_dockerfile: the actual priority logic ──

    #[test]
    fn cli_overrides_everything() {
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

    #[test]
    fn resume_cannot_pick_up_cwd_dockerfile() {
        let tmp = tempfile::tempdir().unwrap();
        let fake_dockerfile = tmp.path().join("Dockerfile");
        std::fs::write(&fake_dockerfile, "FROM alpine").unwrap();

        let path = LaunchPath::ResumeExisting { dockerfile: None };
        assert_eq!(path.resolve_dockerfile(&None), None,
            "ResumeExisting must NEVER auto-detect from cwd");
    }

    #[test]
    fn create_new_can_auto_detect() {
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

    #[test]
    fn determine_nonexistent_session_returns_create_new() {
        let discovered = DiscoveredSession::DoesNotExist(
            SessionName::new("test-nonexistent-launch-path")
        );
        assert!(matches!(discovered, DiscoveredSession::DoesNotExist(_)));
    }

    #[test]
    fn stopped_session_produces_resume_existing() {
        let path = LaunchPath::ResumeExisting { dockerfile: None };
        assert_eq!(path.resolve_dockerfile(&None), None,
            "No dockerfile → should use default image");
    }

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

        let _ = lc.docker_client().remove_image(
            "claude-dev-test-resolve-image",
            None::<bollard::image::RemoveImageOptions>,
            None,
        ).await;
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn resolve_image_default_when_no_dockerfile() {
        let lc = lifecycle::Lifecycle::new().expect("Docker required for this test");
        let name = SessionName::new("test-default-image");
        let result = resolve_image(&lc, &name, &None).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_str(), "ghcr.io/hypermemetic/claude-container:latest");
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn resolve_image_nonexistent_dockerfile_errors() {
        let lc = lifecycle::Lifecycle::new().expect("Docker required");
        let name = SessionName::new("test-bad-dockerfile");
        let bogus = Some(PathBuf::from("/nonexistent/path/Dockerfile"));
        let result = resolve_image(&lc, &name, &bogus).await;

        assert!(result.is_err(), "Non-existent Dockerfile should error");
    }
}
