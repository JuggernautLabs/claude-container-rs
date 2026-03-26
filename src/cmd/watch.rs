use crate::types::*;
use crate::lifecycle;
use crate::session;
use crate::watch;
use colored::Colorize;

pub(crate) async fn cmd_watch(
    name: &SessionName,
    filter: Option<&str>,
    interval_secs: u64,
    command: &[String],
) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    lc.ensure_util_image().await;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    let config = sm.read_or_discover_config(name).await?;

    let mut repo_paths: std::collections::HashMap<String, std::path::PathBuf> = config.projects.iter()
        .filter(|(_, pcfg)| pcfg.role == config::RepoRole::Project)
        .map(|(pname, pcfg)| (pname.clone(), pcfg.path.clone()))
        .collect();

    if let Some(pattern) = filter {
        let re = regex::Regex::new(pattern)?;
        repo_paths.retain(|name, _| re.is_match(name));
    }

    if repo_paths.is_empty() {
        anyhow::bail!("No repos to watch");
    }

    eprintln!("[{}] watching {} repo(s), polling every {}s",
        name.to_string().as_str().blue(),
        repo_paths.len(),
        interval_secs);
    eprintln!("  press Ctrl-C to stop");
    eprintln!();

    let mut watcher = watch::Watcher::new(
        lc.docker_client().clone(),
        name.clone(),
        repo_paths,
        std::time::Duration::from_secs(interval_secs),
    );

    let start = std::time::Instant::now();
    let mut cmd_child: Option<std::process::Child> = None;

    watcher.run(|events, summary| {
        for event in events {
            eprintln!("{}", watch::format_event(event, start));

            if !command.is_empty() && event.source == watch::ChangeSource::Container {
                if let Some(ref mut child) = cmd_child {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let child = std::process::Command::new(&command[0])
                    .args(&command[1..])
                    .spawn();
                match child {
                    Ok(c) => { cmd_child = Some(c); }
                    Err(e) => { eprintln!("  {} command failed: {}", "✗".red(), e); }
                }
            }
        }

        if events.is_empty() {
            // Periodic — just update the summary
        }
    }).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires Docker
    async fn cmd_watch_requires_repos() {
        let name = SessionName::new("test-nonexistent-watch");
        let result = cmd_watch(&name, None, 3, &[]).await;
        assert!(result.is_err());
    }
}
