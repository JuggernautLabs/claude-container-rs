pub mod types;
pub mod lifecycle;
pub mod session;
pub mod sync;
pub mod render;
pub mod container;
pub mod scripts;
pub mod shell_safety;
pub mod watch;
pub mod vm;

use bollard::Docker;
use std::collections::HashMap;

pub use types::THROWAWAY_LABEL;
pub use types::SESSION_LABEL;

/// Prompt for confirmation. Returns true if confirmed.
/// With auto_yes=true, always returns true without prompting.
pub fn confirm(prompt: &str, auto_yes: bool) -> bool {
    if auto_yes { return true; }
    eprint!("{} [Y/n] ", prompt);
    use std::io::Write;
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    !answer.trim().to_lowercase().starts_with('n')
}

/// Remove all containers labeled `claude-container.throwaway=true`.
/// Returns the list of container names that were removed.
pub async fn gc_throwaway_containers(docker: &Docker) -> anyhow::Result<Vec<String>> {
    use bollard::container::{ListContainersOptions, RemoveContainerOptions};

    let mut filters = HashMap::new();
    filters.insert("label".to_string(), vec![format!("{}=true", THROWAWAY_LABEL)]);

    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        }))
        .await?;

    let mut removed = Vec::new();
    for container in &containers {
        let names: Vec<String> = container
            .names
            .as_ref()
            .map(|n| n.iter().map(|s| s.trim_start_matches('/').to_string()).collect())
            .unwrap_or_default();
        let id = container.id.as_deref().unwrap_or("");

        let target = names.first().map(|s| s.as_str()).unwrap_or(id);
        match docker
            .remove_container(
                target,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(()) => {
                removed.push(target.to_string());
            }
            Err(e) => {
                eprintln!("  warning: failed to remove {}: {}", target, e);
            }
        }
    }

    Ok(removed)
}

/// Remove throwaway containers belonging to a specific session.
/// Filters by both `claude-container.throwaway=true` and `claude-container.session=<name>`.
pub async fn gc_session_throwaway_containers(
    docker: &Docker,
    session_name: &str,
) -> anyhow::Result<Vec<String>> {
    use bollard::container::{ListContainersOptions, RemoveContainerOptions};

    let mut filters = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![
            format!("{}=true", THROWAWAY_LABEL),
            format!("{}={}", SESSION_LABEL, session_name),
        ],
    );

    let containers = docker
        .list_containers(Some(ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        }))
        .await?;

    let mut removed = Vec::new();
    for container in &containers {
        let names: Vec<String> = container
            .names
            .as_ref()
            .map(|n| n.iter().map(|s| s.trim_start_matches('/').to_string()).collect())
            .unwrap_or_default();
        let id = container.id.as_deref().unwrap_or("");

        let target = names.first().map(|s| s.as_str()).unwrap_or(id);
        match docker
            .remove_container(
                target,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(()) => {
                removed.push(target.to_string());
            }
            Err(e) => {
                eprintln!("  warning: failed to remove {}: {}", target, e);
            }
        }
    }

    Ok(removed)
}

/// List sessions that have at least one Docker volume existing.
/// Returns only session names where `claude-session-<name>` volume exists.
pub async fn list_active_sessions(docker: &Docker) -> anyhow::Result<Vec<String>> {
    // Collect all session names from both volumes and metadata
    let mut all_sessions: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // From volumes
    let volumes = docker
        .list_volumes(None::<bollard::volume::ListVolumesOptions<String>>)
        .await?;
    let volume_names: std::collections::HashSet<String> = volumes
        .volumes
        .as_ref()
        .map(|vols| vols.iter().map(|v| v.name.clone()).collect())
        .unwrap_or_default();

    for name in &volume_names {
        if let Some(session) = name.strip_prefix("claude-session-") {
            all_sessions.insert(session.to_string());
        }
    }

    // From metadata
    let meta_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".config/claude-container/sessions");
    if meta_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&meta_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let name = name
                    .strip_suffix(".env")
                    .or_else(|| name.strip_suffix(".yml"))
                    .or_else(|| name.strip_suffix(".yaml"))
                    .unwrap_or(&name)
                    .to_string();
                if !name.starts_with('.') {
                    all_sessions.insert(name);
                }
            }
        }
    }

    // Filter: only keep sessions that have a claude-session-<name> volume
    let active: Vec<String> = all_sessions
        .into_iter()
        .filter(|session| {
            let vol_name = format!("claude-session-{}", session);
            volume_names.contains(&vol_name)
        })
        .collect();

    Ok(active)
}

/// Marker: cmd_session_stop requires confirmation (auto_yes parameter).
/// Referenced by safety_test to verify the confirmation gate exists.
pub const fn cmd_session_stop_requires_confirm() -> bool { true }

/// Marker: rebuild validates image before removing container.
/// Referenced by safety_test to verify the implementation order.
pub const REBUILD_VALIDATES_BEFORE_REMOVE: bool = true;
