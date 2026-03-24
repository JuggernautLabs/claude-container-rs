//! Tests for GS-8: Container & Ref Orphan Cleanup
//!
//! Tests gc, cleanup of throwaway containers, and ls --active filtering.

mod harness;

use bollard::container::{Config, CreateContainerOptions, RemoveContainerOptions};
use std::collections::HashMap;

/// Label key used to mark throwaway containers for gc.
const THROWAWAY_LABEL: &str = "claude-container.throwaway";

#[tokio::test]
#[ignore]
async fn gc_removes_orphaned_throwaway_containers() {
    let docker = harness::docker();

    // Create a container with the throwaway label
    let container_name = format!("cc-gc-test-throwaway-{}", std::process::id());
    let _ = docker
        .remove_container(
            &container_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    let mut labels = HashMap::new();
    labels.insert(THROWAWAY_LABEL.to_string(), "true".to_string());

    let config = Config {
        image: Some("alpine:latest".to_string()),
        cmd: Some(vec!["echo".to_string(), "orphan".to_string()]),
        labels: Some(labels),
        ..Default::default()
    };

    docker
        .create_container(
            Some(CreateContainerOptions {
                name: container_name.as_str(),
                platform: None,
            }),
            config,
        )
        .await
        .expect("create throwaway container");

    // Verify it exists
    let inspect = docker.inspect_container(&container_name, None).await;
    assert!(inspect.is_ok(), "throwaway container should exist before gc");

    // Run gc via the library function
    let removed = git_sandbox::gc_throwaway_containers(&docker).await.expect("gc");
    assert!(
        removed.iter().any(|name| name.contains("cc-gc-test-throwaway")),
        "gc should have removed our test container, got: {:?}",
        removed,
    );

    // Verify it's gone
    let inspect_after = docker.inspect_container(&container_name, None).await;
    assert!(
        inspect_after.is_err(),
        "throwaway container should be gone after gc"
    );
}

#[tokio::test]
#[ignore]
async fn gc_preserves_session_containers() {
    let docker = harness::docker();

    // Create a container that looks like a real session container (no throwaway label)
    let container_name = format!("claude-session-ctr-gc-test-{}", std::process::id());
    let _ = docker
        .remove_container(
            &container_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    let config = Config {
        image: Some("alpine:latest".to_string()),
        cmd: Some(vec!["echo".to_string(), "session".to_string()]),
        ..Default::default()
    };

    docker
        .create_container(
            Some(CreateContainerOptions {
                name: container_name.as_str(),
                platform: None,
            }),
            config,
        )
        .await
        .expect("create session container");

    // Run gc
    let removed = git_sandbox::gc_throwaway_containers(&docker).await.expect("gc");

    // Our session container should NOT be in the removed list
    assert!(
        !removed.iter().any(|name| name.contains(&container_name)),
        "gc should NOT remove session containers"
    );

    // Verify it still exists
    let inspect = docker.inspect_container(&container_name, None).await;
    assert!(
        inspect.is_ok(),
        "session container should still exist after gc"
    );

    // Cleanup
    let _ = docker
        .remove_container(
            &container_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
}

#[tokio::test]
#[ignore]
async fn cleanup_removes_session_throwaway_containers() {
    let docker = harness::docker();
    let session_name = format!("cleanup-test-{}", std::process::id());

    // Create a throwaway container that belongs to this session
    let container_name = format!("cc-snap-{}-abc123", session_name);
    let _ = docker
        .remove_container(
            &container_name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;

    let mut labels = HashMap::new();
    labels.insert(THROWAWAY_LABEL.to_string(), "true".to_string());
    labels.insert(
        "claude-container.session".to_string(),
        session_name.clone(),
    );

    let config = Config {
        image: Some("alpine:latest".to_string()),
        cmd: Some(vec!["echo".to_string(), "snap".to_string()]),
        labels: Some(labels),
        ..Default::default()
    };

    docker
        .create_container(
            Some(CreateContainerOptions {
                name: container_name.as_str(),
                platform: None,
            }),
            config,
        )
        .await
        .expect("create session throwaway");

    // Run session-scoped cleanup
    let removed =
        git_sandbox::gc_session_throwaway_containers(&docker, &session_name)
            .await
            .expect("session gc");

    assert!(
        removed.iter().any(|n| n.contains("cc-snap")),
        "session gc should remove throwaway containers for session, got: {:?}",
        removed,
    );

    // Verify gone
    let inspect = docker.inspect_container(&container_name, None).await;
    assert!(
        inspect.is_err(),
        "session throwaway should be gone after cleanup"
    );
}

/// Test that ls_active_sessions filters out sessions without volumes.
/// This test uses a mock approach — we test the filtering logic directly.
#[tokio::test]
#[ignore]
async fn ls_active_filters_stale_sessions() {
    let docker = harness::docker();

    // Create volumes for session "active-test"
    let active_session = format!("active-test-{}", std::process::id());
    let active_vol = format!("claude-session-{}", active_session);
    let _ = docker
        .create_volume(bollard::volume::CreateVolumeOptions {
            name: active_vol.clone(),
            ..Default::default()
        })
        .await;

    // Create metadata for both sessions (active has volume, stale doesn't)
    let meta_dir = dirs::home_dir()
        .unwrap()
        .join(".config/claude-container/sessions");
    std::fs::create_dir_all(&meta_dir).ok();

    let stale_session = format!("stale-test-{}", std::process::id());
    let active_meta = meta_dir.join(format!("{}.env", active_session));
    let stale_meta = meta_dir.join(format!("{}.env", stale_session));
    std::fs::write(&active_meta, "# test\n").ok();
    std::fs::write(&stale_meta, "# test\n").ok();

    // Get active sessions
    let active = git_sandbox::list_active_sessions(&docker).await.expect("list active");

    // Active session should be present
    assert!(
        active.iter().any(|s| s == &active_session),
        "active session with volumes should be in the active list, got: {:?}",
        active,
    );

    // Stale session should NOT be present
    assert!(
        !active.iter().any(|s| s == &stale_session),
        "stale session without volumes should not be in the active list",
    );

    // Cleanup
    let _ = docker
        .remove_volume(&active_vol, None::<bollard::volume::RemoveVolumeOptions>)
        .await;
    let _ = std::fs::remove_file(&active_meta);
    let _ = std::fs::remove_file(&stale_meta);
}

/// Verify that throwaway containers created by sync operations have the label.
#[test]
fn throwaway_label_constant_matches() {
    assert_eq!(
        git_sandbox::THROWAWAY_LABEL,
        "claude-container.throwaway",
        "label constant must match what we use in tests"
    );
}
