use crate::types::*;
use crate::lifecycle;
use colored::Colorize;

pub(crate) async fn cmd_list() -> anyhow::Result<()> {
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
            "running" => "●".green(),
            "stopped" => "○".yellow(),
            _ => "·".dimmed(),
        };

        println!("  {} {:24} {}", marker, name, state.dimmed());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires Docker
    async fn cmd_list_is_callable() {
        // Just verify it compiles and is callable
        let _ = cmd_list().await;
    }

    // Compile-time test: cmd_list exists and has the right return type
    fn _assert_cmd_list_signature() {
        fn _check() -> impl std::future::Future<Output = anyhow::Result<()>> {
            cmd_list()
        }
    }
}
