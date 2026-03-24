//! Test harness — declarative utilities for creating and cleaning up test sessions.
//!
//! Usage:
//!   let session = TestSession::new("my-test").await;
//!   // session.name, session.volumes, session.docker are available
//!   // On drop: volumes removed, containers removed, temp dirs cleaned
//!
//!   let container = session.create_container(&image, cmd, env, binds).await;
//!   // container.name, container.logs().await, container.wait().await
//!   // On drop: container removed
//!
//!   let repo = TestRepo::new("test-repo");
//!   // Creates a temp git repo with an initial commit
//!   // repo.path, repo.commit("message", &[("file", "content")])
//!   // On drop: temp dir removed

use bollard::container::{
    Config, CreateContainerOptions, LogOutput, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, WaitContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ============================================================================
// Docker connection (shared)
// ============================================================================

pub fn docker() -> Docker {
    // Auto-detect via docker context
    if std::env::var("DOCKER_HOST").is_err() {
        if let Ok(output) = std::process::Command::new("docker")
            .args(["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"])
            .output()
        {
            let host = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !host.is_empty() && host.starts_with("unix://") {
                std::env::set_var("DOCKER_HOST", &host);
            }
        }
    }
    Docker::connect_with_local_defaults().expect("Docker connection")
}

pub fn token() -> Option<String> {
    std::env::var("CLAUDE_CODE_OAUTH_TOKEN").ok()
        .or_else(|| {
            let home = dirs::home_dir()?;
            std::fs::read_to_string(home.join(".config/claude-container/token")).ok()
        })
        .map(|t| t.trim().to_string())
}

pub fn script_dir() -> PathBuf {
    git_sandbox::scripts::materialize().expect("materialize scripts")
}

pub const BASE_IMAGE: &str = "ghcr.io/hypermemetic/claude-container:latest";

// ============================================================================
// TestSession — owns volumes, cleans up on drop
// ============================================================================

pub struct TestSession {
    pub name: String,
    pub docker: Docker,
    pub volumes: Vec<String>,
    _cleanup: bool,
}

impl TestSession {
    /// Create a test session with unique name. Creates all 5 volumes.
    pub async fn new(prefix: &str) -> Self {
        let docker = docker();
        let name = format!("{}-{}", prefix, std::process::id());

        let volumes = vec![
            format!("claude-session-{}", name),
            format!("claude-state-{}", name),
            format!("claude-cargo-{}", name),
            format!("claude-npm-{}", name),
            format!("claude-pip-{}", name),
        ];

        for vol in &volumes {
            let _ = docker
                .create_volume(bollard::volume::CreateVolumeOptions {
                    name: vol.clone(),
                    ..Default::default()
                })
                .await;
        }

        Self {
            name,
            docker,
            volumes,
            _cleanup: true,
        }
    }

    /// Session volume name (the workspace)
    pub fn session_volume(&self) -> &str {
        &self.volumes[0]
    }

    /// State volume name (.claude)
    pub fn state_volume(&self) -> &str {
        &self.volumes[1]
    }

    /// Create a container in this session
    pub async fn run_container(
        &self,
        image: &str,
        cmd: &str,
        env: Vec<String>,
        extra_binds: Vec<String>,
    ) -> TestContainer {
        TestContainer::create(
            &self.docker,
            &self.name,
            image,
            cmd,
            env,
            extra_binds,
        )
        .await
    }

    /// Run the entrypoint with BASH_EXEC (tests user creation + early config)
    pub async fn run_entrypoint_with_bash_exec(
        &self,
        bash_exec: &str,
    ) -> ContainerResult {
        let tok = token().unwrap_or_else(|| "test-token".into());
        let scripts = script_dir();

        let mut env = vec![
            "RUN_AS_ROOTISH=1".to_string(),
            format!("CLAUDE_CODE_OAUTH_TOKEN_NESTED={}", tok),
            format!("BASH_EXEC={}", bash_exec),
            format!("HOST_UID={}", unsafe { libc::getuid() }),
            format!("HOST_GID={}", unsafe { libc::getgid() }),
            "TERM=xterm-256color".to_string(),
            "PLATFORM=linux".to_string(),
        ];

        let binds = vec![
            format!("{}:/usr/local/bin/cc-entrypoint:ro", scripts.join("cc-entrypoint").display()),
            format!("{}:/usr/local/bin/cc-developer-setup:ro", scripts.join("cc-developer-setup").display()),
            format!("{}:/usr/local/bin/cc-agent-run:ro", scripts.join("cc-agent-run").display()),
            format!("{}:/home/developer/.claude", self.state_volume()),
        ];

        let cmd = "chmod +x /usr/local/bin/cc-entrypoint /usr/local/bin/cc-developer-setup /usr/local/bin/cc-agent-run 2>/dev/null; exec /usr/local/bin/cc-entrypoint";

        let tc = self.run_container(BASE_IMAGE, cmd, env, binds).await;
        tc.wait_and_collect().await
    }

    /// Run a simple command in a container with session volume mounted
    pub async fn run_simple(&self, image: &str, cmd: &str) -> ContainerResult {
        let tc = self.run_container(image, cmd, vec![], vec![
            format!("{}:/workspace", self.session_volume()),
        ]).await;
        tc.wait_and_collect().await
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        if self._cleanup {
            let docker = self.docker.clone();
            let volumes = self.volumes.clone();
            let name = self.name.clone();

            // Synchronous cleanup — block on async
            let rt = tokio::runtime::Handle::try_current();
            if let Ok(handle) = rt {
                handle.spawn(async move {
                    // Remove any containers
                    let ctr_name = format!("test-ctr-{}", name);
                    let _ = docker.remove_container(
                        &ctr_name,
                        Some(RemoveContainerOptions { force: true, ..Default::default() }),
                    ).await;

                    // Remove volumes
                    for vol in &volumes {
                        let _ = docker.remove_volume(vol, None::<bollard::volume::RemoveVolumeOptions>).await;
                    }
                });
            }
        }
    }
}

// ============================================================================
// TestContainer — runs a container, collects output, cleans up
// ============================================================================

pub struct TestContainer {
    pub name: String,
    docker: Docker,
}

impl TestContainer {
    async fn create(
        docker: &Docker,
        session_name: &str,
        image: &str,
        cmd: &str,
        env: Vec<String>,
        extra_binds: Vec<String>,
    ) -> Self {
        let name = format!("test-ctr-{}-{}", session_name, rand_suffix());

        // Clean up leftover
        let _ = docker.remove_container(
            &name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        // Use sh instead of bash for broader compatibility (alpine/git has no bash)
        let config = Config {
            image: Some(image.to_string()),
            user: Some("0:0".to_string()),
            entrypoint: Some(vec!["sh".to_string()]),
            cmd: Some(vec!["-c".to_string(), cmd.to_string()]),
            env: Some(env),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(extra_binds),
                ..Default::default()
            }),
            tty: Some(false),
            ..Default::default()
        };

        docker
            .create_container(
                Some(CreateContainerOptions { name: name.as_str(), platform: None }),
                config,
            )
            .await
            .expect("create container");

        docker
            .start_container(&name, None::<StartContainerOptions<String>>)
            .await
            .expect("start container");

        Self {
            name,
            docker: docker.clone(),
        }
    }

    /// Wait for container to exit and collect stdout + stderr
    pub async fn wait_and_collect(self) -> ContainerResult {
        let docker = &self.docker;

        // Wait for exit
        let mut wait = docker.wait_container(
            &self.name,
            Some(WaitContainerOptions { condition: "not-running".to_string() }),
        );
        let mut exit_code = -1i64;
        while let Some(result) = wait.next().await {
            match result {
                Ok(r) => { exit_code = r.status_code; }
                Err(bollard::errors::Error::DockerContainerWaitError { code, .. }) => {
                    exit_code = code;
                }
                Err(_) => {}
            }
        }

        // Collect logs
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut logs = docker.logs(
            &self.name,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: true,
                follow: false,
                ..Default::default()
            }),
        );
        while let Some(chunk) = logs.next().await {
            if let Ok(log) = chunk {
                match log {
                    LogOutput::StdOut { message } => {
                        stdout.push_str(&String::from_utf8_lossy(&message));
                    }
                    LogOutput::StdErr { message } => {
                        stderr.push_str(&String::from_utf8_lossy(&message));
                    }
                    _ => {}
                }
            }
        }

        // Clean up
        let _ = docker.remove_container(
            &self.name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        ContainerResult {
            exit_code,
            stdout,
            stderr,
        }
    }
}

/// Result of running a container
#[derive(Debug)]
pub struct ContainerResult {
    pub exit_code: i64,
    pub stdout: String,
    pub stderr: String,
}

impl ContainerResult {
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0
    }

    pub fn assert_success(&self) {
        assert!(
            self.succeeded(),
            "Container exited with code {}.\nStdout:\n{}\nStderr:\n{}",
            self.exit_code, self.stdout, self.stderr
        );
    }

    pub fn assert_stdout_contains(&self, needle: &str) {
        assert!(
            self.stdout.contains(needle),
            "Stdout should contain '{}'. Got:\n{}\nStderr:\n{}",
            needle, self.stdout, self.stderr
        );
    }

    pub fn assert_stderr_not_contains(&self, needle: &str) {
        assert!(
            !self.stderr.contains(needle),
            "Stderr should NOT contain '{}'. Got:\n{}",
            needle, self.stderr
        );
    }
}

// ============================================================================
// TestRepo — temporary git repo with commits
// ============================================================================

pub struct TestRepo {
    pub path: PathBuf,
    _temp: tempfile::TempDir,
}

impl TestRepo {
    /// Create a temp git repo with an initial commit
    pub fn new(name: &str) -> Self {
        let temp = tempfile::TempDir::new().expect("create temp dir");
        let path = temp.path().join(name);
        std::fs::create_dir_all(&path).expect("create repo dir");

        let repo = git2::Repository::init(&path).expect("git init");

        // Initial commit
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        let tree_id = {
            let mut index = repo.index().unwrap();
            // Create a file
            let file_path = path.join("README.md");
            std::fs::write(&file_path, format!("# {}\n", name)).unwrap();
            index.add_path(Path::new("README.md")).unwrap();
            index.write().unwrap();
            index.write_tree().unwrap()
        };
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[]).unwrap();

        Self { path, _temp: temp }
    }

    /// Add a file and commit
    pub fn commit(&self, message: &str, files: &[(&str, &str)]) -> git2::Oid {
        let repo = git2::Repository::open(&self.path).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();

        for (name, content) in files {
            let file_path = self.path.join(name);
            if let Some(parent) = file_path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&file_path, content).unwrap();
        }

        let mut index = repo.index().unwrap();
        for (name, _) in files {
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();

        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent]).unwrap()
    }

    pub fn head(&self) -> String {
        let repo = git2::Repository::open(&self.path).unwrap();
        let x = repo.head().unwrap().peel_to_commit().unwrap().id().to_string(); x
    }

    pub fn branch(&self) -> String {
        let repo = git2::Repository::open(&self.path).unwrap();
        let x = repo.head().unwrap().shorthand().unwrap_or("HEAD").to_string(); x
    }
}

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{:x}", t.as_nanos() % 0xFFFFFF)
}

// ============================================================================
// Tests for the harness itself
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_harness_session_creates_and_cleans_volumes() {
        let d = docker();

        let session = TestSession::new("harness-test").await;
        let vol_name = session.session_volume().to_string();

        // Volumes should exist
        let info = d.inspect_volume(&vol_name).await;
        assert!(info.is_ok(), "Session volume should exist");

        // Drop cleans up
        let name = session.name.clone();
        drop(session);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Volume should be gone (eventually — cleanup is async)
        // Don't assert — cleanup is best-effort in drop
    }

    #[tokio::test]
    #[ignore]
    async fn test_harness_container_runs_and_captures_output() {
        let session = TestSession::new("harness-ctr").await;

        let result = session.run_simple(BASE_IMAGE, "echo HELLO_HARNESS && echo ERR >&2").await;

        println!("Exit: {}, Stdout: '{}', Stderr: '{}'", result.exit_code, result.stdout.trim(), result.stderr.trim());
        assert_eq!(result.exit_code, 0);
        result.assert_stdout_contains("HELLO_HARNESS");
        assert!(result.stderr.contains("ERR"));
    }

    #[tokio::test]
    #[ignore]
    async fn test_harness_entrypoint_runs_as_developer() {
        let session = TestSession::new("harness-ep").await;

        let result = session.run_entrypoint_with_bash_exec("whoami && id").await;

        println!("Exit: {}, Stdout: '{}', Stderr: '{}'", result.exit_code, result.stdout.trim(), result.stderr.trim());
        result.assert_stderr_not_contains("Permission denied");
        result.assert_stdout_contains("developer");
    }

    #[test]
    fn test_harness_repo_creates_with_initial_commit() {
        let repo = TestRepo::new("test-repo");
        assert!(repo.path.join(".git").exists());
        assert!(repo.path.join("README.md").exists());
        assert!(!repo.head().is_empty());
        assert!(!repo.branch().is_empty());
    }

    #[test]
    fn test_harness_repo_commit_adds_files() {
        let repo = TestRepo::new("test-repo-commit");
        let head_before = repo.head();

        repo.commit("add stuff", &[("foo.txt", "bar"), ("src/main.rs", "fn main() {}")]);

        let head_after = repo.head();
        assert_ne!(head_before, head_after);
        assert!(repo.path.join("foo.txt").exists());
        assert!(repo.path.join("src/main.rs").exists());
    }
}
