use crate::types::*;
use crate::lifecycle;
use crate::session;
use crate::sync;
use crate::render;
use crate::shell_safety;
use std::path::PathBuf;
use colored::Colorize;

use super::confirm;
use super::CliRepoRole;

pub(crate) async fn cmd_session_show(name: &SessionName, filter: Option<&str>) -> anyhow::Result<()> {
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

pub(crate) async fn cmd_session_add_repo(name: &SessionName, paths: &[PathBuf]) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    // Verify session exists
    let discovered = sm.discover(name).await?;
    match &discovered {
        DiscoveredSession::DoesNotExist(_) => {
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
                eprintln!("  {} Not a git repo: {}", "⚠".yellow(), p.display());
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
        eprintln!("  {} {} → {}", "+".blue(), repo_name, path.display());
    }

    // Clone repos into the session volume
    let lc = lifecycle::Lifecycle::new()?;
    let engine = sync::SyncEngine::new(lc.docker_client().clone());
    for (i, (repo_name, path)) in repos_to_add.iter().enumerate() {
        eprintln!("  Cloning [{}/{}] {}...", i + 1, repos_to_add.len(), repo_name);
        engine.clone_into_volume(name, repo_name, path, None).await?;
    }
    eprintln!("  {} {} repo(s) added", "✓".green(), repos_to_add.len());

    Ok(())
}

pub(crate) async fn cmd_session_exec(name: &SessionName, as_root: bool, command: &[String]) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let docker = lc.docker_client();
    let container_name = name.container_name();

    // Check container is running
    match lc.inspect_container(&container_name).await? {
        docker::ContainerState::Running { .. } => {}
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

pub(crate) async fn cmd_session_stop(name: &SessionName, auto_yes: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let container_name = name.container_name();

    match lc.inspect_container(&container_name).await? {
        docker::ContainerState::Running { .. } => {
            if !confirm(&format!("  Stop container '{}'?", name), auto_yes) {
                eprintln!("  Aborted.");
                return Ok(());
            }
            eprintln!("  Stopping {}...", container_name);
            lc.docker_client().stop_container(
                container_name.as_str(),
                Some(bollard::container::StopContainerOptions { t: 10 }),
            ).await?;
            eprintln!("  {} Stopped.", "✓".green());
        }
        docker::ContainerState::Stopped { .. } => {
            eprintln!("  Already stopped.");
        }
        docker::ContainerState::NotFound { .. } => {
            eprintln!("  No container.");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_session_rebuild(name: &SessionName, auto_yes: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let container_name = name.container_name();

    // Check current state
    match lc.inspect_container(&container_name).await? {
        docker::ContainerState::NotFound { .. } => {
            eprintln!("  No container to rebuild.");
            return Ok(());
        }
        docker::ContainerState::Running { .. } => {
            if !confirm("  Container is running. Stop and rebuild?", auto_yes) {
                eprintln!("  Aborted.");
                return Ok(());
            }
        }
        _ => {}
    }

    // Build image FIRST — only remove container after successful build.
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
                eprintln!("  {} Image built successfully.", "✓".green());
            }
        }
    }

    eprintln!("  Removing container {}...", container_name);
    lc.remove_container(&container_name).await?;
    eprintln!("  {} Container removed. Volumes preserved.", "✓".green());

    eprintln!("  Run `git-sandbox start -s {}` to launch.", name);
    Ok(())
}

pub(crate) async fn cmd_session_cleanup(name: &SessionName, auto_yes: bool) -> anyhow::Result<()> {
    if !confirm("  Remove stale markers from session volume?", auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    let lc = lifecycle::Lifecycle::new()?;

    let volume = name.session_volume();
    let container_name = format!("cc-cleanup-{}", name);

    let _ = lc.docker_client().remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    let script = "rm -f /session/.reconcile-complete /session/.merge-into-summary /session/.merge-into-branch /session/.sync-summary /session/.sync-branch 2>/dev/null; echo CLEANED";
    use docker::{throwaway_config, VolumeMount, RunAs};
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

    eprintln!("  {} Stale markers removed from session volume.", "✓".green());
    Ok(())
}

pub(crate) async fn cmd_session_verify(name: &SessionName) -> anyhow::Result<()> {
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

    let mounts: Vec<crate::types::docker::VolumeMount> = volumes.iter()
        .map(|(vol, mount)| crate::types::docker::VolumeMount::ReadOnly { source: vol.to_string(), target: mount.to_string() })
        .collect();

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

    let config = crate::types::docker::throwaway_config(
        "alpine/git", &script, &mounts, &crate::types::docker::RunAs::developer(), name,
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
                eprintln!("  {} {} — {} file(s), all owned by you", "✓".green(), parts[1], parts[3]);
            }
        } else if line.starts_with("PROBLEM|") {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() >= 4 {
                eprintln!("  {} {} — {}/{} file(s) wrong owner", "✗".red(), parts[1], parts[2], parts[3]);
                problems += 1;
            }
        } else if !line.trim().is_empty() {
            eprintln!("    {}", line.trim().dimmed());
        }
    }

    eprintln!();
    if problems > 0 {
        eprintln!("  {} {} volume(s) have ownership problems.", "⚠".yellow(), problems);
        eprintln!("  Run `git-sandbox session -s {} fix` to repair.", name);
    } else {
        eprintln!("  {} All volumes clean.", "✓".green());
    }

    Ok(())
}

pub(crate) async fn cmd_session_fix(name: &SessionName, auto_yes: bool) -> anyhow::Result<()> {
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

    let mounts: Vec<crate::types::docker::VolumeMount> = volumes.iter()
        .map(|(vol, mount)| crate::types::docker::VolumeMount::Writable { source: vol.to_string(), target: mount.to_string() })
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

    let config = crate::types::docker::throwaway_config(
        "alpine/git", &script, &mounts, &crate::types::docker::RunAs::Root, name,
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

    eprintln!("  {} All volumes fixed.", "✓".green());
    Ok(())
}

pub(crate) async fn cmd_session_set_role(name: &SessionName, repo_pattern: &str, role: CliRepoRole) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    let mut config = sm.read_or_discover_config(name).await?;

    let target_role = match role {
        CliRepoRole::Project => config::RepoRole::Project,
        CliRepoRole::Dependency => config::RepoRole::Dependency,
    };

    let re = regex::Regex::new(repo_pattern)
        .map_err(|e| anyhow::anyhow!("Invalid pattern '{}': {}", repo_pattern, e))?;

    let mut matched = 0;
    for (pname, pcfg) in config.projects.iter_mut() {
        if re.is_match(pname) {
            pcfg.role = target_role.clone();
            matched += 1;
            eprintln!("  {} {} → {}", "✓".green(), pname, target_role);
        }
    }

    if matched == 0 {
        anyhow::bail!("No repos match '{}'", repo_pattern);
    }

    sm.write_config(name, &config).await?;

    eprintln!("  {} {} repo(s) updated", "✓".green(), matched);
    Ok(())
}

pub(crate) async fn cmd_session_set_dir(name: &SessionName, target: Option<&str>) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let engine = sync::SyncEngine::new(lc.docker_client().clone());

    match target {
        Some(dir) => {
            engine.write_main_project(name, dir).await?;
            eprintln!("  {} Main project set to '{}'", "✓".green(), dir);
        }
        None => {
            let volume = name.session_volume();
            let container_name = format!("cc-setdir-{}", name);
            let _ = lc.docker_client().remove_container(
                &container_name,
                Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
            ).await;

            let config = crate::types::docker::throwaway_config(
                "alpine/git", "rm -f /session/.main-project",
                &[crate::types::docker::VolumeMount::Writable { source: volume.to_string(), target: "/session".into() }],
                &crate::types::docker::RunAs::developer(), name,
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

            eprintln!("  {} Main project cleared (defaults to /workspace)", "✓".green());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires Docker
    async fn session_show_is_callable() {
        let name = SessionName::new("test-nonexistent");
        // Should fail gracefully since session doesn't exist
        let _ = cmd_session_show(&name, None).await;
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn session_exec_requires_running_container() {
        let name = SessionName::new("test-nonexistent-exec");
        let result = cmd_session_exec(&name, false, &["echo".into(), "hello".into()]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn session_stop_with_nonexistent() {
        let name = SessionName::new("test-nonexistent-stop");
        // Should complete without error (prints "No container.")
        let _ = cmd_session_stop(&name, true).await;
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn session_add_repo_requires_existing_session() {
        let name = SessionName::new("test-nonexistent-add");
        let result = cmd_session_add_repo(&name, &[]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn session_rebuild_nonexistent() {
        let name = SessionName::new("test-nonexistent-rebuild");
        let _ = cmd_session_rebuild(&name, true).await;
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn session_verify_is_callable() {
        let name = SessionName::new("test-nonexistent-verify");
        let _ = cmd_session_verify(&name).await;
    }
}
