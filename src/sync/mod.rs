//! Sync engine — snapshot container state, classify repos, build sync plans.
//!
//! Uses bollard for container-side snapshots (one docker run per scan)
//! and git2 for all host-side git operations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bollard::container::{
    Config as ContainerConfig, CreateContainerOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, WaitContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use git2::Repository;

use crate::types::{
    Ancestry, CommitHash, ContainerError, ContentComparison, DiffSummary, ExtractResult, GitSide,
    MergeOutcome, PairRelation, Plan, RepoPair, RepoSyncAction, RepoSyncResult, SessionName,
    LegState, PullAction, PushAction, SessionSyncPlan, SquashState, SyncResult,
    TargetAheadKind, VolumeRepo,
};

/// Git utility image used for container-side scans.
const GIT_UTIL_IMAGE: &str = "alpine/git";

/// Result of merging a host branch into a container repo.
#[derive(Debug)]
pub enum MergeIntoResult {
    /// Merge completed cleanly (auto-committed)
    CleanMerge,
    /// Merge has conflicts — <<<<<<< markers left in working tree
    Conflict { files: Vec<String> },
    /// Already up to date (host branch is ancestor of container HEAD)
    AlreadyUpToDate,
}

fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:x}", t.as_nanos() % 0xFFFFFFFF, c)
}

/// Shell script injected into the scanner container.
/// Outputs lines: `name|head|dirty|merging|gitsize`
const SCAN_SCRIPT: &str = r#"
git config --global --add safe.directory "*"
for d in /session/*/ /session/*/*/; do
    [ -d "$d/.git" ] || continue
    name="${d#/session/}"; name="${name%/}"
    head=$(cd "$d" && git rev-parse HEAD 2>/dev/null | head -1)
    case "$head" in
        [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]*) ;;
        *) continue ;;
    esac
    dirty=$(cd "$d" && git status --porcelain 2>/dev/null | wc -l | tr -d ' ')
    merging="no"; [ -f "$d/.git/MERGE_HEAD" ] && merging="yes"
    rebasing="no"; [ -d "$d/.git/rebase-merge" ] || [ -d "$d/.git/rebase-apply" ] && rebasing="yes"
    gitsize=$(du -sm "$d/.git" 2>/dev/null | cut -f1)
    echo "$name|$head|$dirty|$merging|$rebasing|${gitsize:-0}"
done
"#;

pub struct SyncEngine {
    docker: Docker,
}

impl SyncEngine {
    pub fn new(docker: Docker) -> Self {
        Self { docker }
    }

    // ========================================================================
    // Snapshot: read all container-side state via one docker run
    // ========================================================================

    /// Scan every git repo in the session volume.
    /// Runs a throwaway container that mounts the session volume read-only
    /// and outputs `name|head|dirty|merging|rebasing|gitsize` per repo.
    pub async fn snapshot(
        &self,
        session: &SessionName,
        _target_branch: &str,
    ) -> Result<Vec<VolumeRepo>, ContainerError> {
        let volume_name = session.session_volume();

        let container_name = format!("cc-snap-{}-{}", session, rand_suffix());
        // Clean up any leftover container with similar name
        let _ = self.docker.remove_container(&container_name, Some(RemoveContainerOptions { force: true, ..Default::default() })).await;
        let config = ContainerConfig {
            image: Some(GIT_UTIL_IMAGE.to_string()),
            entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
            cmd: Some(vec![SCAN_SCRIPT.to_string()]),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(vec![format!("{}:/session:ro", volume_name)]),
                ..Default::default()
            }),
            ..Default::default()
        };

        // Create container
        let opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };
        self.docker.create_container(Some(opts), config).await?;

        // Start it
        self.docker
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await?;

        // Wait for exit
        let mut wait_stream = self
            .docker
            .wait_container(&container_name, None::<WaitContainerOptions<String>>);
        while let Some(result) = wait_stream.next().await {
            // We just need to consume the stream; exit code checked implicitly
            let _ = result?;
        }

        // Collect stdout logs
        let mut log_stream = self.docker.logs(
            &container_name,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: false,
                follow: false,
                ..Default::default()
            }),
        );

        let mut stdout = String::new();
        while let Some(chunk) = log_stream.next().await {
            if let Ok(output) = chunk {
                stdout.push_str(&output.to_string());
            }
        }

        // Remove the container
        self.docker
            .remove_container(
                &container_name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .ok(); // best-effort cleanup

        // Parse output
        let repos = parse_scan_output(&stdout);
        Ok(repos)
    }

    // ========================================================================
    // Classify: determine the (container, host) GitSide pair for one repo
    // ========================================================================

    /// Build a `RepoPair` for a single repo by comparing container state
    /// (from snapshot) against host state (via git2).
    ///
    /// The triple: container HEAD vs session branch HEAD vs target branch HEAD.
    pub fn classify_repo(
        &self,
        repo_name: &str,
        container: &VolumeRepo,
        host_path: &Path,
        session_name: &str,
        target_branch: &str,
    ) -> RepoPair {
        // Container side
        let container_side = volume_repo_to_gitside(container);

        // Host side — open repo via git2
        let (host_side, relation, target_head, session_to_target) = match Repository::open(host_path) {
            Ok(repo) => {
                let host_side = read_host_side(&repo, session_name);
                let relation = match (container_side.head(), host_side.head()) {
                    (Some(c_head), Some(h_head)) => {
                        Some(self.compute_relation(&repo, c_head, h_head, session_name))
                    }
                    _ => None,
                };

                // Read target branch state (the third leg of the triple)
                let (t_head, st_rel) = if !target_branch.is_empty() {
                    read_target_state(&repo, session_name, target_branch, host_side.head(), self, host_path)
                } else {
                    (None, None)
                };

                (host_side, relation, t_head, st_rel)
            }
            Err(_) => {
                if host_path.exists() {
                    (
                        GitSide::NotARepo {
                            path: host_path.to_path_buf(),
                        },
                        None,
                        None,
                        None,
                    )
                } else {
                    (GitSide::Missing, None, None, None)
                }
            }
        };

        RepoPair {
            name: repo_name.to_string(),
            container: container_side,
            host: host_side,
            relation,
            target_head,
            session_to_target,
        }
    }

    // ========================================================================
    // Plan: snapshot + classify everything → SessionSyncPlan
    // ========================================================================

    /// Build a full sync plan: snapshot the container, classify each repo,
    /// compute diffs where needed, return a `Plan<SessionSyncPlan>`.
    pub async fn plan_sync(
        &self,
        session: &SessionName,
        target_branch: &str,
        repo_configs: &BTreeMap<String, PathBuf>,
    ) -> Result<Plan<SessionSyncPlan>, ContainerError> {
        // Step 1: snapshot container state
        let volume_repos = self.snapshot(session, target_branch).await?;

        // Step 2: classify each repo
        let mut repo_actions = Vec::new();
        for vr in &volume_repos {
            let host_path = match repo_configs.get(&vr.name) {
                Some(p) => p.clone(),
                None => continue, // no host mapping — skip
            };

            let pair = self.classify_repo(
                &vr.name,
                vr,
                &host_path,
                session.as_str(),
                target_branch,
            );

            let state = pair.repo_state();

            // Step 3: compute diffs using two-leg state
            let pull_act = state.pull_action();
            let push_act = state.push_action();

            let (outbound_diff, inbound_diff) = match (&state.extraction, &push_act) {
                // Container ahead → outbound diff (session..container)
                (LegState::ContainerAhead { .. } | LegState::Unknown, _) => {
                    let diff = pair.host.head().and_then(|h_head| {
                        pair.container.head()
                            .and_then(|c_head| self.compute_diff(&host_path, h_head, c_head))
                    });
                    (diff, None)
                }
                // Session ahead → inbound diff (container..session)
                (LegState::SessionAhead { .. }, _) => {
                    let diff = pair.container.head().and_then(|c_head| {
                        pair.host.head()
                            .and_then(|h_head| self.compute_diff(&host_path, c_head, h_head))
                    });
                    (None, diff)
                }
                // Diverged → both diffs from merge-base
                (LegState::Diverged { .. }, _) => {
                    let outbound = pair.host.head().and_then(|h_head| {
                        pair.relation.as_ref().and_then(|rel| match &rel.ancestry {
                            Ancestry::Diverged { merge_base, .. } => merge_base.as_ref()
                                .and_then(|mb| self.compute_diff(&host_path, mb, h_head)),
                            _ => None,
                        })
                    });
                    let inbound = pair.container.head().and_then(|c_head| {
                        pair.relation.as_ref().and_then(|rel| match &rel.ancestry {
                            Ancestry::Diverged { merge_base, .. } => merge_base.as_ref()
                                .and_then(|mb| self.compute_diff(&host_path, mb, c_head)),
                            _ => None,
                        })
                    });
                    (outbound, inbound)
                }
                // Extraction in sync but push has inject work → inbound diff
                (_, PushAction::Inject { .. }) => {
                    let diff = pair.target_head.as_ref().and_then(|t_head| {
                        pair.host.head()
                            .and_then(|s_head| self.compute_diff(&host_path, s_head, t_head))
                    });
                    (None, diff)
                }
                _ => (None, None),
            };

            // Trial merge for Extract/Reconcile/MergeToTarget
            let trial_conflicts = match &pull_act {
                PullAction::Extract { .. } | PullAction::Reconcile => {
                    match (pair.host.head(), pair.container.head()) {
                        (Some(ours), Some(theirs)) => self.trial_merge(&host_path, ours, theirs),
                        _ => None,
                    }
                }
                PullAction::MergeToTarget { .. } => {
                    match (pair.target_head.as_ref(), pair.host.head()) {
                        (Some(target), Some(session)) => self.trial_merge(&host_path, target, session),
                        _ => None,
                    }
                }
                _ => None,
            };

            // Session→target diff for MergeToTarget
            let session_to_target_diff = match &pull_act {
                PullAction::MergeToTarget { .. } => {
                    pair.target_head.as_ref().and_then(|t_head| {
                        pair.host.head().and_then(|s_head| {
                            self.compute_diff(&host_path, t_head, s_head)
                        })
                    })
                }
                _ => None,
            };

            repo_actions.push(RepoSyncAction {
                repo_name: vr.name.clone(),
                host_path: Some(host_path.clone()),
                state,
                container_head: pair.container.head().cloned(),
                session_head: pair.host.head().cloned(),
                target_head: pair.target_head.clone(),
                outbound_diff,
                inbound_diff,
                trial_conflicts,
                session_to_target_diff,
            });
        }

        let plan_action = SessionSyncPlan {
            session_name: session.clone(),
            target_branch: target_branch.to_string(),
            repo_actions,
        };

        let has_work = plan_action.has_work();
        let description = format_plan_description(&plan_action);

        Ok(Plan {
            action: plan_action,
            description,
            destructive: has_work,
        })
    }

    // ========================================================================
    // Trial merge: in-memory, zero side effects
    // ========================================================================

    /// Perform an in-memory trial merge to detect conflicts without touching
    /// the working directory, index, or any refs. Uses git2::merge_trees()
    /// which operates purely on git objects — safe even on crash.
    ///
    /// Returns None if either commit isn't available locally, or Some with
    /// the list of conflicting file paths (empty = clean merge).
    pub fn trial_merge(
        &self,
        host_path: &Path,
        ours_hash: &CommitHash,
        theirs_hash: &CommitHash,
    ) -> Option<Vec<String>> {
        let repo = Repository::open(host_path).ok()?;
        let ours_oid = git2::Oid::from_str(ours_hash.as_str()).ok()?;
        let theirs_oid = git2::Oid::from_str(theirs_hash.as_str()).ok()?;

        let ours_commit = repo.find_commit(ours_oid).ok()?;
        let theirs_commit = repo.find_commit(theirs_oid).ok()?;

        let merge_base_oid = repo.merge_base(ours_oid, theirs_oid).ok()?;
        let base_commit = repo.find_commit(merge_base_oid).ok()?;

        let base_tree = base_commit.tree().ok()?;
        let ours_tree = ours_commit.tree().ok()?;
        let theirs_tree = theirs_commit.tree().ok()?;

        let mut merge_opts = git2::MergeOptions::new();
        let index = repo.merge_trees(&base_tree, &ours_tree, &theirs_tree, Some(&mut merge_opts)).ok()?;

        if index.has_conflicts() {
            let conflicts: Vec<String> = index
                .conflicts().ok()?
                .filter_map(|c| c.ok())
                .filter_map(|c| {
                    c.our.or(c.their).or(c.ancestor)
                        .and_then(|entry| String::from_utf8(entry.path).ok())
                })
                .collect();
            Some(conflicts)
        } else {
            Some(vec![]) // clean merge
        }
    }

    // ========================================================================
    // Diff: squash-aware diff between two refs
    // ========================================================================

    /// Compute a `DiffSummary` between two commits in a host repo.
    /// Returns `None` if either commit is not reachable from the host.
    pub fn compute_diff(
        &self,
        repo_path: &Path,
        from: &CommitHash,
        to: &CommitHash,
    ) -> Option<DiffSummary> {
        use crate::types::action::{FileDiff, FileStatus};

        let repo = Repository::open(repo_path).ok()?;

        let from_oid = git2::Oid::from_str(from.as_str()).ok()?;
        let to_oid = git2::Oid::from_str(to.as_str()).ok()?;

        let from_commit = repo.find_commit(from_oid).ok()?;
        let to_commit = repo.find_commit(to_oid).ok()?;

        let from_tree = from_commit.tree().ok()?;
        let to_tree = to_commit.tree().ok()?;

        let diff = repo
            .diff_tree_to_tree(Some(&from_tree), Some(&to_tree), None)
            .ok()?;
        let stats = diff.stats().ok()?;

        // Collect per-file diffs
        let mut files = Vec::new();
        let num_deltas = diff.deltas().len();
        for i in 0..num_deltas {
            let delta = diff.get_delta(i).unwrap();
            let path = delta.new_file().path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "?".into());

            let status = match delta.status() {
                git2::Delta::Added => FileStatus::Added,
                git2::Delta::Deleted => FileStatus::Deleted,
                git2::Delta::Renamed => {
                    let old = delta.old_file().path()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    FileStatus::Renamed(old)
                }
                _ => FileStatus::Modified,
            };

            // Per-file line counts via Patch
            let (ins, del) = git2::Patch::from_diff(&diff, i).ok()
                .flatten()
                .map(|patch| {
                    let (_, i, d) = patch.line_stats().unwrap_or((0, 0, 0));
                    (i as u32, d as u32)
                })
                .unwrap_or((0, 0));

            files.push(FileDiff {
                path,
                status,
                insertions: ins,
                deletions: del,
            });
        }

        Some(DiffSummary {
            files_changed: stats.files_changed() as u32,
            insertions: stats.insertions() as u32,
            deletions: stats.deletions() as u32,
            files,
        })
    }

    // ========================================================================
    // Ancestry check
    // ========================================================================

    /// Determine the ancestry relationship between two commits.
    pub fn check_ancestry(
        &self,
        repo_path: &Path,
        a: &CommitHash,
        b: &CommitHash,
    ) -> Ancestry {
        let repo = match Repository::open(repo_path) {
            Ok(r) => r,
            Err(_) => return Ancestry::Unknown,
        };

        let a_oid = match git2::Oid::from_str(a.as_str()) {
            Ok(o) => o,
            Err(_) => return Ancestry::Unknown,
        };
        let b_oid = match git2::Oid::from_str(b.as_str()) {
            Ok(o) => o,
            Err(_) => return Ancestry::Unknown,
        };

        if a_oid == b_oid {
            return Ancestry::Same;
        }

        // Check if either is an ancestor of the other
        let a_is_ancestor = repo.graph_descendant_of(b_oid, a_oid).unwrap_or(false);
        let b_is_ancestor = repo.graph_descendant_of(a_oid, b_oid).unwrap_or(false);

        match (a_is_ancestor, b_is_ancestor) {
            (true, false) => {
                // a is ancestor of b → b is ahead
                let count = count_commits_between(&repo, a_oid, b_oid).unwrap_or(1);
                Ancestry::ContainerBehind {
                    host_ahead: count,
                }
            }
            (false, true) => {
                // b is ancestor of a → a is ahead
                let count = count_commits_between(&repo, b_oid, a_oid).unwrap_or(1);
                Ancestry::ContainerAhead {
                    container_ahead: count,
                }
            }
            (false, false) => {
                // Neither is ancestor — diverged
                let merge_base = repo.merge_base(a_oid, b_oid).ok();
                let container_ahead = merge_base
                    .and_then(|mb| count_commits_between(&repo, mb, a_oid).ok())
                    .unwrap_or(1);
                let host_ahead = merge_base
                    .and_then(|mb| count_commits_between(&repo, mb, b_oid).ok())
                    .unwrap_or(1);
                Ancestry::Diverged {
                    container_ahead,
                    host_ahead,
                    merge_base: merge_base.map(|mb| CommitHash::new(mb.to_string())),
                }
            }
            (true, true) => {
                // Both ancestors of each other → same (shouldn't happen if oids differ)
                Ancestry::Same
            }
        }
    }

    // ========================================================================
    // Internal: compute PairRelation
    // ========================================================================

    fn compute_relation(
        &self,
        repo: &Repository,
        c_head: &CommitHash,
        h_head: &CommitHash,
        session_name: &str,
    ) -> PairRelation {
        let repo_path = repo.workdir().unwrap_or_else(|| repo.path());
        let ancestry = self.check_ancestry(repo_path, c_head, h_head);
        let content = compute_content_comparison(repo, c_head, h_head);
        let squash = read_squash_state(repo, session_name, c_head);
        let target_ahead = compute_target_ahead(repo, session_name, &ancestry, &squash);

        PairRelation {
            ancestry,
            content,
            squash,
            target_ahead,
        }
    }

    // ========================================================================
    // Extract: container volume → host session branch (via git bundle)
    // ========================================================================

    /// Extract a repo from the container's session volume to the host.
    ///
    /// Creates a throwaway container that mounts the session volume and a temp
    /// directory, runs `git bundle create` inside the repo, then on the host
    /// fetches from the bundle and creates/updates the session branch.
    pub async fn extract(
        &self,
        session: &SessionName,
        repo_name: &str,
        host_path: &Path,
        session_branch: &str,
    ) -> Result<ExtractResult, ContainerError> {
        let volume_name = session.session_volume();

        // Create a temp dir on the host to receive the bundle.
        // MUST be under $HOME — Colima/Docker Desktop only mount user dirs,
        // not /var/folders or /tmp which are macOS-specific.
        let bundle_base = dirs::cache_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".cache")))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("git-sandbox/bundles");
        std::fs::create_dir_all(&bundle_base).map_err(|e| ContainerError::Io(e))?;
        let bundle_dir = tempfile::tempdir_in(&bundle_base).map_err(|e| ContainerError::Io(e))?;
        let bundle_host_path = bundle_dir.path().join("repo.bundle");

        let container_name = format!("cc-extract-{}-{}", session, rand_suffix());
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        // Script: cd into the repo inside /session, create bundle
        // Use --all to bundle everything (handles detached HEAD, any branch)
        let script = format!(
            r#"
set -e
git config --global --add safe.directory "*"
cd "/session/{repo_name}" || {{ echo "FAIL: repo not found at /session/{repo_name}"; exit 1; }}
echo "HEAD=$(git rev-parse HEAD)"
echo "branch=$(git symbolic-ref --short HEAD 2>/dev/null || echo detached)"
git bundle create /bundles/repo.bundle --all 2>&1
echo "BUNDLE_OK"
"#,
            repo_name = repo_name,
        );

        let config = ContainerConfig {
            image: Some(GIT_UTIL_IMAGE.to_string()),
            entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
            cmd: Some(vec![script]),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(vec![
                    format!("{}:/session:ro", volume_name),
                    format!("{}:/bundles", bundle_dir.path().display()),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };
        self.docker.create_container(Some(opts), config).await?;

        self.docker
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await?;

        // Wait for exit
        let mut wait_stream = self
            .docker
            .wait_container(&container_name, None::<WaitContainerOptions<String>>);
        let mut exit_code: i64 = -1;
        while let Some(result) = wait_stream.next().await {
            match result {
                Ok(resp) => {
                    exit_code = resp.status_code;
                }
                Err(bollard::errors::Error::DockerContainerWaitError { code, .. }) => {
                    exit_code = code;
                }
                Err(_) => {}
            }
        }

        // Collect logs for diagnostics
        let mut log_output = String::new();
        let mut log_stream = self.docker.logs(
            &container_name,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: true,
                follow: false,
                ..Default::default()
            }),
        );
        while let Some(chunk) = log_stream.next().await {
            if let Ok(output) = chunk {
                log_output.push_str(&output.to_string());
            }
        }

        // Clean up the container
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        if exit_code != 0 {
            return Err(ContainerError::BundleFailed {
                repo: repo_name.to_string(),
                reason: format!("git bundle exited {}: {}", exit_code, log_output.lines().last().unwrap_or("unknown")),
            });
        }

        // Verify the bundle file exists
        if !bundle_host_path.exists() {
            return Err(ContainerError::BundleFailed {
                repo: repo_name.to_string(),
                reason: format!("bundle file not created. Container output:\n{}", log_output),
            });
        }

        // On the host: fetch from the bundle using git CLI (more reliable than libgit2 for bundles)
        let repo = Repository::open(host_path).map_err(|_| ContainerError::NotAGitRepo(host_path.to_path_buf()))?;

        let bundle_path_str = bundle_host_path.to_string_lossy().to_string();

        // Use git CLI to fetch — libgit2 bundle support is flaky
        let fetch_output = std::process::Command::new("git")
            .args(["-C", &host_path.to_string_lossy(), "fetch", &bundle_path_str, "HEAD"])
            .output()
            .map_err(|e| ContainerError::Io(e))?;

        if !fetch_output.status.success() {
            // Try fetching all refs instead of HEAD
            let fetch_all = std::process::Command::new("git")
                .args(["-C", &host_path.to_string_lossy(), "fetch", &bundle_path_str, "+refs/*:refs/cc-bundle/*"])
                .output()
                .map_err(|e| ContainerError::Io(e))?;

            if !fetch_all.status.success() {
                let stderr = String::from_utf8_lossy(&fetch_all.stderr);
                return Err(ContainerError::FetchFailed {
                    repo: repo_name.to_string(),
                    reason: format!("git fetch from bundle failed: {}", stderr.lines().last().unwrap_or("unknown")),
                });
            }
        }

        // Resolve FETCH_HEAD (set by git fetch)
        let fetch_head = repo.find_reference("FETCH_HEAD")
            .map_err(|_| ContainerError::BranchCreateFailed {
                repo: repo_name.to_string(),
                reason: "FETCH_HEAD not set after bundle fetch".to_string(),
            })?;
        let fetch_commit = fetch_head.peel_to_commit()?;
        let new_head_oid = fetch_commit.id();
        let new_head = CommitHash::new(new_head_oid.to_string());

        // Create or update the session branch to point at FETCH_HEAD
        let session_ref = format!("refs/heads/{}", session_branch);
        repo.reference(&session_ref, new_head_oid, true, "cc: extract from container")?;

        // Count commits (from merge base with current HEAD if it exists, else all)
        let commit_count = {
            let mut revwalk = repo.revwalk()?;
            revwalk.push(new_head_oid)?;
            // Try to limit the count to something reasonable
            revwalk.set_sorting(git2::Sort::TOPOLOGICAL)?;
            let mut count = 0u32;
            for oid_result in revwalk {
                if oid_result.is_ok() {
                    count += 1;
                    if count >= 10000 {
                        break; // safety cap
                    }
                }
            }
            count
        };

        // bundle_dir drops here, cleaning up the temp directory

        Ok(ExtractResult {
            commit_count,
            new_head,
        })
    }

    // ========================================================================
    // Merge: host session branch → host target branch
    // ========================================================================

    /// Merge the session branch into the target branch on the host.
    ///
    /// Supports fast-forward, squash merge (with squash-base tracking), and
    /// regular merge. Returns `MergeOutcome` describing what happened.
    pub fn merge(
        &self,
        host_path: &Path,
        session_branch: &str,
        target_branch: &str,
        squash: bool,
    ) -> Result<MergeOutcome, ContainerError> {
        let repo = Repository::open(host_path)
            .map_err(|_| ContainerError::NotAGitRepo(host_path.to_path_buf()))?;

        // Find session branch
        let session_ref_name = format!("refs/heads/{}", session_branch);
        let session_ref = repo.find_reference(&session_ref_name).map_err(|_| {
            ContainerError::BranchNotFound {
                repo: host_path.display().to_string(),
                branch: session_branch.to_string(),
            }
        })?;
        let session_commit = session_ref.peel_to_commit()?;
        let session_oid = session_commit.id();

        // Find target branch
        let target_ref_name = format!("refs/heads/{}", target_branch);
        let target_ref = repo.find_reference(&target_ref_name).map_err(|_| {
            ContainerError::BranchNotFound {
                repo: host_path.display().to_string(),
                branch: target_branch.to_string(),
            }
        })?;
        let target_commit = target_ref.peel_to_commit()?;
        let target_oid = target_commit.id();

        // Check if already up to date
        let merge_base_oid = repo.merge_base(target_oid, session_oid).ok();

        if session_oid == target_oid {
            return Ok(MergeOutcome::AlreadyUpToDate);
        }

        if merge_base_oid == Some(session_oid) {
            // Session is an ancestor of target — target is already ahead
            return Ok(MergeOutcome::AlreadyUpToDate);
        }

        // Check for fast-forward: target is an ancestor of session
        let is_ff = merge_base_oid == Some(target_oid);

        if !squash && is_ff {
            // Fast-forward: just move the target ref
            let ff_count = count_commits_between(&repo, target_oid, session_oid).unwrap_or(1);
            repo.reference(&target_ref_name, session_oid, true, "cc: fast-forward merge")?;

            // Also update HEAD / working tree if target is checked out
            if let Ok(head_ref) = repo.head() {
                if head_ref.name() == Some(&target_ref_name) {
                    repo.checkout_head(Some(
                        git2::build::CheckoutBuilder::new().force(),
                    ))?;
                }
            }

            return Ok(MergeOutcome::FastForward { commits: ff_count });
        }

        // Squash merge
        if squash {
            // Determine the effective base for squash
            let squash_base_ref_name = format!(
                "refs/claude-container/squash-base/{}",
                session_branch,
            );
            let effective_base = repo
                .find_reference(&squash_base_ref_name)
                .ok()
                .and_then(|r| r.peel_to_commit().ok())
                .and_then(|c| {
                    // Verify squash-base is an ancestor of session HEAD
                    let base_oid = c.id();
                    if repo.graph_descendant_of(session_oid, base_oid).unwrap_or(false) {
                        Some(base_oid)
                    } else {
                        // Stale squash-base (session was likely rebased in container)
                        eprintln!(
                            "  note: squash-base {} reset (session rebased), using merge-base",
                            &c.id().to_string()[..7.min(c.id().to_string().len())],
                        );
                        None
                    }
                })
                .or(merge_base_oid);

            let effective_base_oid = match effective_base {
                Some(oid) => oid,
                None => {
                    // No common ancestor at all — this shouldn't normally happen
                    return Ok(MergeOutcome::Blocked(
                        crate::types::MergeBlocker::NoSessionBranch,
                    ));
                }
            };

            let new_commits = count_commits_between(&repo, effective_base_oid, session_oid)
                .unwrap_or(0);

            if new_commits == 0 {
                return Ok(MergeOutcome::AlreadyUpToDate);
            }

            // Checkout target branch
            repo.set_head(&target_ref_name)?;
            repo.checkout_head(Some(
                git2::build::CheckoutBuilder::new().force(),
            ))?;

            // Perform the merge (squash style: merge into index, don't commit yet)
            let annotated = repo.find_annotated_commit(session_oid)?;
            let mut merge_opts = git2::MergeOptions::new();
            repo.merge(&[&annotated], Some(&mut merge_opts), None)?;

            // Check for conflicts
            let index = repo.index()?;
            if index.has_conflicts() {
                let conflict_files: Vec<String> = index
                    .conflicts()?
                    .filter_map(|c| c.ok())
                    .filter_map(|c| {
                        c.our
                            .or(c.their)
                            .or(c.ancestor)
                            .and_then(|entry| String::from_utf8(entry.path).ok())
                    })
                    .collect();

                // Clean up: clear merge state AND restore working tree to pre-merge state.
                // Without checkout_head, conflict markers stay in the working tree
                // and every subsequent check sees "host has uncommitted changes."
                repo.cleanup_state()?;
                repo.checkout_head(Some(
                    git2::build::CheckoutBuilder::new().force(),
                ))?;

                return Ok(MergeOutcome::Conflict { files: conflict_files });
            }

            // Write the tree from the merged index
            let mut index = repo.index()?;
            let tree_id = index.write_tree()?;
            let tree = repo.find_tree(tree_id)?;

            // Create a squash commit (single parent = target)
            let sig = repo.signature().unwrap_or_else(|_| {
                git2::Signature::now("git-sandbox", "git-sandbox@local").unwrap()
            });
            let message = format!(
                "squash: {} commit(s) from session branch {}",
                new_commits, session_branch,
            );
            let parent = repo.find_commit(target_oid)?;
            let new_commit_oid = repo.commit(
                Some("HEAD"),
                &sig,
                &sig,
                &message,
                &tree,
                &[&parent],
            )?;

            // Update the target branch ref to the new commit
            repo.reference(&target_ref_name, new_commit_oid, true, "cc: squash merge")?;

            // Update squash-base ref to session HEAD (so next squash starts from here)
            repo.reference(
                &squash_base_ref_name,
                session_oid,
                true,
                "cc: update squash-base",
            )?;

            // Clean up merge state
            repo.cleanup_state()?;

            return Ok(MergeOutcome::SquashMerge {
                commits: new_commits,
                squash_base: CommitHash::new(session_oid.to_string()),
            });
        }

        // Regular merge (non-ff, non-squash)
        // Checkout target branch
        repo.set_head(&target_ref_name)?;
        repo.checkout_head(Some(
            git2::build::CheckoutBuilder::new().force(),
        ))?;

        let annotated = repo.find_annotated_commit(session_oid)?;
        let mut merge_opts = git2::MergeOptions::new();
        repo.merge(&[&annotated], Some(&mut merge_opts), None)?;

        let index = repo.index()?;
        if index.has_conflicts() {
            let conflict_files: Vec<String> = index
                .conflicts()?
                .filter_map(|c| c.ok())
                .filter_map(|c| {
                    c.our
                        .or(c.their)
                        .or(c.ancestor)
                        .and_then(|entry| String::from_utf8(entry.path).ok())
                })
                .collect();

            repo.cleanup_state()?;
            repo.checkout_head(Some(
                git2::build::CheckoutBuilder::new().force(),
            ))?;
            return Ok(MergeOutcome::Conflict { files: conflict_files });
        }

        // Commit the merge
        let mut index = repo.index()?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;

        let sig = repo.signature().unwrap_or_else(|_| {
            git2::Signature::now("git-sandbox", "git-sandbox@local").unwrap()
        });
        let message = format!(
            "Merge session branch {} into {}",
            session_branch, target_branch,
        );
        let parent_target = repo.find_commit(target_oid)?;
        let parent_session = repo.find_commit(session_oid)?;
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            &message,
            &tree,
            &[&parent_target, &parent_session],
        )?;

        // Update target ref
        let new_head = repo.head()?.peel_to_commit()?.id();
        repo.reference(&target_ref_name, new_head, true, "cc: merge commit")?;

        repo.cleanup_state()?;

        Ok(MergeOutcome::CleanMerge)
    }

    // ========================================================================
    // Inject: host branch → container volume (via bind mount + git fetch)
    // ========================================================================

    /// Inject changes from a host branch into the container's repo.
    // ========================================================================
    // Clone: host repo → session volume (initial session creation)
    // ========================================================================

    /// Clone a repo from the host into the session volume.
    /// Used during session creation — the repo doesn't exist in the volume yet.
    ///
    /// Runs a throwaway container with alpine/git that:
    /// 1. Bind-mounts the host repo at /upstream (read-only)
    /// 2. Mounts the session volume at /session
    /// 3. Runs `git clone` from /upstream into /session/<repo_name>
    pub async fn clone_into_volume(
        &self,
        session: &SessionName,
        repo_name: &str,
        host_path: &Path,
        branch: Option<&str>,
    ) -> Result<(), ContainerError> {
        let volume_name = session.session_volume();
        let container_name = format!("cc-clone-{}-{}", session, rand_suffix());
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        let host_path_str = host_path.to_string_lossy().to_string();

        let branch_flag = match branch {
            Some(b) => format!("--branch \"{}\"", b),
            None => String::new(),
        };

        // Run as root (volume mount is root-owned), then chown to host UID
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };

        let script = format!(
            r#"
export HOME=/tmp
git config --global --add safe.directory "*"
git clone {branch_flag} "/upstream" "/session/{repo_name}" || exit 1
chown -R {uid}:{gid} "/session/{repo_name}" 2>/dev/null || true
cd "/session/{repo_name}" || exit 1
echo "Cloned $(git rev-parse --short HEAD) on $(git symbolic-ref --short HEAD 2>/dev/null || echo 'detached')"
"#,
            branch_flag = branch_flag,
            repo_name = repo_name,
            uid = uid,
            gid = gid,
        );

        let config = ContainerConfig {
            image: Some(GIT_UTIL_IMAGE.to_string()),
            entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
            cmd: Some(vec![script]),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(vec![
                    format!("{}:/session", volume_name),
                    format!("{}:/upstream:ro", host_path_str),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };
        self.docker.create_container(Some(opts), config).await?;

        self.docker
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await?;

        // Wait for exit (handle DockerContainerWaitError as exit code)
        let mut wait_stream = self
            .docker
            .wait_container(&container_name, None::<WaitContainerOptions<String>>);
        let mut exit_code: i64 = -1;
        while let Some(result) = wait_stream.next().await {
            match result {
                Ok(resp) => exit_code = resp.status_code,
                Err(bollard::errors::Error::DockerContainerWaitError { code, .. }) => {
                    exit_code = code;
                }
                Err(_) => {}
            }
        }

        // Collect logs
        let mut log_output = String::new();
        let mut log_stream = self.docker.logs(
            &container_name,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: true,
                follow: false,
                ..Default::default()
            }),
        );
        while let Some(chunk) = log_stream.next().await {
            if let Ok(output) = chunk {
                log_output.push_str(&output.to_string());
            }
        }

        // Clean up
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        if exit_code != 0 {
            return Err(ContainerError::FetchFailed {
                repo: repo_name.to_string(),
                reason: format!(
                    "clone exited with code {}: {}",
                    exit_code,
                    log_output.lines().last().unwrap_or("unknown error"),
                ),
            });
        }

        Ok(())
    }

    /// Write a .main-project marker into the session volume.
    pub async fn write_main_project(
        &self,
        session: &SessionName,
        main_project: &str,
    ) -> Result<(), ContainerError> {
        let volume_name = session.session_volume();
        let container_name = format!("cc-marker-{}-{}", session, rand_suffix());
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };

        let config = ContainerConfig {
            image: Some(GIT_UTIL_IMAGE.to_string()),
            user: Some(format!("{}:{}", uid, gid)),
            entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
            cmd: Some(vec![format!("echo '{}' > /session/.main-project", main_project)]),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(vec![format!("{}:/session", volume_name)]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let opts = CreateContainerOptions { name: &container_name, platform: None };
        self.docker.create_container(Some(opts), config).await?;
        self.docker.start_container(&container_name, None::<StartContainerOptions<String>>).await?;

        let mut wait = self.docker.wait_container(&container_name, None::<WaitContainerOptions<String>>);
        while let Some(_) = wait.next().await {}

        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        Ok(())
    }

    // ========================================================================
    // Inject: host branch → container volume (push into existing repo)
    // ========================================================================

    ///
    /// Mounts the host repo read-only into a throwaway container alongside
    /// the session volume, then fetches and merges inside the container.
    pub async fn inject(
        &self,
        session: &SessionName,
        repo_name: &str,
        host_path: &Path,
        branch: &str,
    ) -> Result<(), ContainerError> {
        let volume_name = session.session_volume();
        let container_name = format!("cc-inject-{}-{}", session, rand_suffix());
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        let host_path_str = host_path.to_string_lossy().to_string();

        let script = format!(
            r#"
git config --global --add safe.directory "*"
cd "/session/{repo_name}" || exit 1
git remote add _cc_upstream "/upstream" 2>/dev/null || git remote set-url _cc_upstream "/upstream"
git fetch _cc_upstream "{branch}" || exit 1
git merge "_cc_upstream/{branch}" --no-edit || exit 1
git remote remove _cc_upstream 2>/dev/null
"#,
            repo_name = repo_name,
            branch = branch,
        );

        use crate::types::docker::{throwaway_config, VolumeMount, RunAs};
        let config = throwaway_config(
            GIT_UTIL_IMAGE,
            &script,
            &[
                VolumeMount::Writable { source: volume_name.to_string(), target: "/session".into() },
                VolumeMount::ReadOnly { source: host_path_str, target: "/upstream".into() },
            ],
            &RunAs::developer(),
            session,
        );

        let opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };
        self.docker.create_container(Some(opts), config).await?;

        self.docker
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await?;

        // Wait for exit
        let mut wait_stream = self
            .docker
            .wait_container(&container_name, None::<WaitContainerOptions<String>>);
        let mut exit_code: i64 = -1;
        while let Some(result) = wait_stream.next().await {
            match result {
                Ok(resp) => {
                    exit_code = resp.status_code;
                }
                Err(e) => {
                    let _ = self.docker.remove_container(
                        &container_name,
                        Some(RemoveContainerOptions { force: true, ..Default::default() }),
                    ).await;
                    return Err(ContainerError::Docker(e));
                }
            }
        }

        // Collect logs for diagnostics
        let mut log_output = String::new();
        if exit_code != 0 {
            let mut log_stream = self.docker.logs(
                &container_name,
                Some(LogsOptions::<String> {
                    stdout: true,
                    stderr: true,
                    follow: false,
                    ..Default::default()
                }),
            );
            while let Some(chunk) = log_stream.next().await {
                if let Ok(output) = chunk {
                    log_output.push_str(&output.to_string());
                }
            }
        }

        // Clean up
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        if exit_code != 0 {
            return Err(ContainerError::InjectionFailed {
                repo: repo_name.to_string(),
                reason: format!(
                    "inject (git fetch+merge) exited with code {}: {}",
                    exit_code,
                    log_output.lines().last().unwrap_or("unknown error"),
                ),
            });
        }

        Ok(())
    }

    // ========================================================================
    // Merge-into: host branch → container repo (with conflict preservation)
    // ========================================================================

    /// Merge a host branch INTO a container repo, preserving conflict markers.
    /// Unlike inject (which uses --no-edit and fails on conflict), this leaves
    /// <<<<<<< markers in the working tree for Claude to resolve.
    pub async fn merge_into_volume(
        &self,
        session: &SessionName,
        repo_name: &str,
        host_path: &Path,
        target_branch: &str,
    ) -> Result<MergeIntoResult, ContainerError> {
        let volume_name = session.session_volume();
        let container_name = format!("cc-mergein-{}-{}", session, rand_suffix());
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        let host_path_str = host_path.to_string_lossy().to_string();
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };

        let script = format!(
            r#"
export HOME=/tmp
git config --global --add safe.directory "*"
git config --global user.email "git-sandbox@local"
git config --global user.name "git-sandbox"
cd "/session/{repo_name}" 2>/dev/null || {{ echo "ERROR|cd failed"; exit 0; }}
git remote remove _upstream 2>/dev/null || true
git remote add _upstream "/upstream"
fetch_out=$(git fetch _upstream "{target_branch}" 2>&1) || {{
    echo "ERROR|fetch failed: $(echo "$fetch_out" | tr '\n' ' ')"
    git remote remove _upstream 2>/dev/null || true
    exit 0
}}
merge_out=$(git merge "_upstream/{target_branch}" --no-edit 2>&1)
merge_rc=$?
git remote remove _upstream 2>/dev/null || true
if echo "$merge_out" | grep -qE "CONFLICT|Automatic merge failed"; then
    echo "CONFLICT"
    git diff --name-only --diff-filter=U 2>/dev/null
elif [ $merge_rc -ne 0 ]; then
    echo "ERROR|$(echo "$merge_out" | tr '\n' ' ')"
elif echo "$merge_out" | grep -q "Already up to date"; then
    echo "UPTODATE"
else
    echo "MERGED"
fi
"#,
            repo_name = repo_name,
            target_branch = target_branch,
        );

        // Run as root (volume may be root-owned), chown conflicts would be worse
        let config = ContainerConfig {
            image: Some(GIT_UTIL_IMAGE.to_string()),
            user: Some(format!("{}:{}", uid, gid)),
            entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
            cmd: Some(vec![script]),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(vec![
                    format!("{}:/session", volume_name),
                    format!("{}:/upstream:ro", host_path_str),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let opts = CreateContainerOptions { name: &container_name, platform: None };
        self.docker.create_container(Some(opts), config).await?;
        self.docker.start_container(&container_name, None::<StartContainerOptions<String>>).await?;

        // Wait
        let mut wait_stream = self.docker.wait_container(&container_name, None::<WaitContainerOptions<String>>);
        while let Some(result) = wait_stream.next().await {
            match result {
                Ok(_) => {}
                Err(bollard::errors::Error::DockerContainerWaitError { .. }) => {}
                Err(_) => {}
            }
        }

        // Collect output
        let mut stdout = String::new();
        let mut log_stream = self.docker.logs(
            &container_name,
            Some(LogsOptions::<String> { stdout: true, stderr: true, follow: false, ..Default::default() }),
        );
        while let Some(chunk) = log_stream.next().await {
            if let Ok(output) = chunk { stdout.push_str(&output.to_string()); }
        }

        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        // Parse output
        let first_line = stdout.lines().next().unwrap_or("").trim();
        match first_line {
            "MERGED" => Ok(MergeIntoResult::CleanMerge),
            "UPTODATE" => Ok(MergeIntoResult::AlreadyUpToDate),
            "CONFLICT" => {
                let files: Vec<String> = stdout.lines().skip(1)
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                Ok(MergeIntoResult::Conflict { files })
            }
            other if other.starts_with("ERROR|") => {
                Err(ContainerError::InjectionFailed {
                    repo: repo_name.to_string(),
                    reason: other.strip_prefix("ERROR|").unwrap_or(other).to_string(),
                })
            }
            _ => {
                Err(ContainerError::InjectionFailed {
                    repo: repo_name.to_string(),
                    reason: format!("unexpected output: {}", stdout.lines().take(3).collect::<Vec<_>>().join(" ")),
                })
            }
        }
    }

    // ========================================================================
    // Execute: orchestrate a full sync plan
    // ========================================================================

    /// Execute a full bidirectional sync: push phase first, then pull phase.
    pub async fn execute_sync(
        &self,
        session: &SessionName,
        plan: SessionSyncPlan,
        repo_configs: &BTreeMap<String, PathBuf>,
    ) -> Result<SyncResult, ContainerError> {
        let target_branch = plan.target_branch.clone();
        let session_branch = session.to_string();
        let mut results = Vec::new();

        // Push phase first: all injects
        for action in &plan.repo_actions {
            let push = action.state.push_action();
            if matches!(push, PushAction::Skip) { continue; }
            let host_path = match repo_configs.get(&action.repo_name) {
                Some(p) => p.clone(),
                None => continue,
            };
            let result = self.dispatch_push(session, &action.repo_name, &host_path, &target_branch, push).await;
            results.push(result);
        }

        // Pull phase: all extracts + merges
        for action in &plan.repo_actions {
            let pull = action.state.pull_action();
            if matches!(pull, PullAction::Skip) { continue; }
            let host_path = match repo_configs.get(&action.repo_name) {
                Some(p) => p.clone(),
                None => continue,
            };
            let result = self.dispatch_pull(session, &action.repo_name, &host_path, &session_branch, &target_branch, pull).await;
            results.push(result);
        }

        // Skipped repos
        for action in &plan.repo_actions {
            if !action.state.has_work() {
                results.push(RepoSyncResult::Skipped {
                    repo_name: action.repo_name.clone(),
                    reason: "up to date".to_string(),
                });
            }
        }

        Ok(SyncResult {
            session_name: session.clone(),
            results,
        })
    }

    /// Execute only push actions from a plan.
    pub async fn execute_push(
        &self,
        session: &SessionName,
        plan: SessionSyncPlan,
        repo_configs: &BTreeMap<String, PathBuf>,
    ) -> Result<SyncResult, ContainerError> {
        let target_branch = plan.target_branch.clone();
        let mut results = Vec::new();

        for action in &plan.repo_actions {
            let push = action.state.push_action();
            let host_path = match repo_configs.get(&action.repo_name) {
                Some(p) => p.clone(),
                None => {
                    results.push(RepoSyncResult::Skipped {
                        repo_name: action.repo_name.clone(),
                        reason: "no host path mapping".to_string(),
                    });
                    continue;
                }
            };
            let result = self.dispatch_push(session, &action.repo_name, &host_path, &target_branch, push).await;
            results.push(result);
        }

        Ok(SyncResult { session_name: session.clone(), results })
    }

    /// Dispatch a single PushAction — typed, no string direction.
    async fn dispatch_push(
        &self,
        session: &SessionName,
        repo_name: &str,
        host_path: &Path,
        target_branch: &str,
        action: PushAction,
    ) -> RepoSyncResult {
        match action {
            PushAction::Skip => RepoSyncResult::Skipped {
                repo_name: repo_name.to_string(),
                reason: "up to date".to_string(),
            },
            PushAction::Inject { .. } => {
                match self.inject(session, repo_name, host_path, target_branch).await {
                    Ok(()) => RepoSyncResult::Pushed { repo_name: repo_name.to_string() },
                    Err(e) => RepoSyncResult::Failed { repo_name: repo_name.to_string(), error: e.to_string() },
                }
            }
            PushAction::PushToContainer => {
                match self.inject(session, repo_name, host_path, target_branch).await {
                    Ok(()) => RepoSyncResult::Pushed { repo_name: repo_name.to_string() },
                    Err(e) => RepoSyncResult::Failed { repo_name: repo_name.to_string(), error: e.to_string() },
                }
            }
            PushAction::Blocked(reason) => RepoSyncResult::Skipped {
                repo_name: repo_name.to_string(),
                reason: format!("blocked: {}", reason),
            },
        }
    }

    /// Dispatch a single PullAction — typed, no string direction.
    async fn dispatch_pull(
        &self,
        session: &SessionName,
        repo_name: &str,
        host_path: &Path,
        session_branch: &str,
        target_branch: &str,
        action: PullAction,
    ) -> RepoSyncResult {
        match action {
            PullAction::Skip => RepoSyncResult::Skipped {
                repo_name: repo_name.to_string(),
                reason: "up to date".to_string(),
            },
            PullAction::Extract { .. } => {
                match self.execute_pull_one(session, repo_name, host_path, session_branch, target_branch).await {
                    Ok(r) => r,
                    Err(e) => RepoSyncResult::Failed { repo_name: repo_name.to_string(), error: e.to_string() },
                }
            }
            PullAction::CloneToHost => {
                match self.extract(session, repo_name, host_path, session_branch).await {
                    Ok(extract) => RepoSyncResult::ClonedToHost { repo_name: repo_name.to_string(), extract },
                    Err(e) => RepoSyncResult::Failed { repo_name: repo_name.to_string(), error: e.to_string() },
                }
            }
            PullAction::MergeToTarget { .. } => {
                match self.merge(host_path, session_branch, target_branch, true) {
                    Ok(merge) => {
                        if let MergeOutcome::Conflict { files } = &merge {
                            RepoSyncResult::Conflicted { repo_name: repo_name.to_string(), files: files.clone() }
                        } else {
                            RepoSyncResult::Merged { repo_name: repo_name.to_string(), merge }
                        }
                    }
                    Err(e) => RepoSyncResult::Failed { repo_name: repo_name.to_string(), error: e.to_string() },
                }
            }
            PullAction::Reconcile => {
                // inject host → container, then extract + merge back
                match self.inject(session, repo_name, host_path, target_branch).await {
                    Ok(()) => {
                        match self.execute_pull_one(session, repo_name, host_path, session_branch, target_branch).await {
                            Ok(r) => r,
                            Err(e) => RepoSyncResult::Failed {
                                repo_name: repo_name.to_string(),
                                error: format!("reconcile pull phase failed: {}", e),
                            },
                        }
                    }
                    Err(e) => RepoSyncResult::Failed {
                        repo_name: repo_name.to_string(),
                        error: format!("reconcile inject phase failed: {}", e),
                    },
                }
            }
            PullAction::Blocked(reason) => RepoSyncResult::Skipped {
                repo_name: repo_name.to_string(),
                reason: format!("blocked: {}", reason),
            },
        }
    }

    // ========================================================================
    // Internal: execute a pull (extract + merge) for one repo
    // ========================================================================

    async fn execute_pull_one(
        &self,
        session: &SessionName,
        repo_name: &str,
        host_path: &Path,
        session_branch: &str,
        target_branch: &str,
    ) -> Result<RepoSyncResult, ContainerError> {
        // Step 1: extract container → host session branch
        let extract = self.extract(session, repo_name, host_path, session_branch).await?;

        // Step 2: merge session branch → target branch (squash by default)
        let merge = self.merge(host_path, session_branch, target_branch, true)?;

        // If the merge resulted in conflicts, return a Conflicted result
        if let MergeOutcome::Conflict { files } = &merge {
            return Ok(RepoSyncResult::Conflicted {
                repo_name: repo_name.to_string(),
                files: files.clone(),
            });
        }

        Ok(RepoSyncResult::Pulled {
            repo_name: repo_name.to_string(),
            extract,
            merge,
        })
    }
}

// ============================================================================
// Free functions — parsing, git2 helpers
// ============================================================================

/// Parse the scanner container output into `VolumeRepo` entries.
fn parse_scan_output(output: &str) -> Vec<VolumeRepo> {
    let mut repos = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(6, '|').collect();
        if parts.len() < 5 {
            continue;
        }
        let name = parts[0].to_string();
        let head = CommitHash::new(parts[1]);
        let dirty_files = parts[2].parse::<u32>().unwrap_or(0);
        let merging = parts[3] == "yes";
        // parts[4] is rebasing (we track it but VolumeRepo doesn't have it — fold into merging)
        let rebasing = parts.get(4).map_or(false, |v| *v == "yes");
        let gitsize_idx = if parts.len() >= 6 { 5 } else { 4 };
        let git_size_mb = parts
            .get(gitsize_idx)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        repos.push(VolumeRepo {
            name,
            head,
            dirty_files,
            merging: merging || rebasing,
            git_size_mb,
        });
    }
    repos
}

/// Convert a `VolumeRepo` (container scan result) into a `GitSide`.
fn volume_repo_to_gitside(vr: &VolumeRepo) -> GitSide {
    if vr.merging {
        GitSide::Merging {
            head: vr.head.clone(),
        }
    } else if vr.dirty_files > 0 {
        GitSide::Dirty {
            head: vr.head.clone(),
            dirty_files: vr.dirty_files,
        }
    } else {
        GitSide::Clean {
            head: vr.head.clone(),
        }
    }
}

/// Read the target branch HEAD and compute session→target relationship.
/// Returns (target_head, session_to_target_relation).
fn read_target_state(
    repo: &Repository,
    session_name: &str,
    target_branch: &str,
    session_head: Option<&CommitHash>,
    engine: &SyncEngine,
    host_path: &Path,
) -> (Option<CommitHash>, Option<crate::types::git::SessionTargetRelation>) {
    let target_ref = format!("refs/heads/{}", target_branch);
    let target_oid = match repo.find_reference(&target_ref) {
        Ok(r) => match r.peel_to_commit() {
            Ok(c) => c.id(),
            Err(_) => return (None, None),
        },
        Err(_) => return (None, None),
    };
    let target_head = CommitHash::new(target_oid.to_string());

    let session_head = match session_head {
        Some(h) => h,
        None => return (Some(target_head), None),
    };

    let session_ref = format!("refs/heads/{}", session_name);
    let session_oid = match repo.find_reference(&session_ref) {
        Ok(r) => match r.peel_to_commit() {
            Ok(c) => c.id(),
            Err(_) => return (Some(target_head), None),
        },
        Err(_) => return (Some(target_head), None),
    };

    if session_oid == target_oid {
        return (Some(target_head), Some(crate::types::git::SessionTargetRelation {
            ancestry: Ancestry::Same,
            content: ContentComparison::Identical,
        }));
    }

    // Compute ancestry between session and target
    let repo_path = repo.workdir().unwrap_or_else(|| repo.path());
    let ancestry = engine.check_ancestry(repo_path, session_head, &target_head);

    // Compute content comparison
    let content = compute_content_comparison(repo, session_head, &target_head);

    (Some(target_head), Some(crate::types::git::SessionTargetRelation {
        ancestry,
        content,
    }))
}

/// Read the host-side git state for the session branch.
fn read_host_side(repo: &Repository, session_name: &str) -> GitSide {
    // Try to find the session branch on the host
    let ref_name = format!("refs/heads/{}", session_name);
    let head = match repo.find_reference(&ref_name) {
        Ok(reference) => match reference.peel_to_commit() {
            Ok(commit) => CommitHash::new(commit.id().to_string()),
            Err(_) => return GitSide::Missing,
        },
        Err(_) => return GitSide::Missing,
    };

    // Check for dirty state on the host workdir
    let statuses = match repo.statuses(None) {
        Ok(s) => s,
        Err(_) => {
            return GitSide::Clean { head };
        }
    };

    let dirty_count = statuses
        .iter()
        .filter(|s| {
            !s.status().is_ignored()
        })
        .count() as u32;

    if dirty_count > 0 {
        GitSide::Dirty {
            head,
            dirty_files: dirty_count,
        }
    } else {
        GitSide::Clean { head }
    }
}

/// Compare the tree content of two commits (ignoring history).
fn compute_content_comparison(
    repo: &Repository,
    a: &CommitHash,
    b: &CommitHash,
) -> ContentComparison {
    let result = (|| -> Option<ContentComparison> {
        let a_oid = git2::Oid::from_str(a.as_str()).ok()?;
        let b_oid = git2::Oid::from_str(b.as_str()).ok()?;

        let a_commit = repo.find_commit(a_oid).ok()?;
        let b_commit = repo.find_commit(b_oid).ok()?;

        let a_tree = a_commit.tree().ok()?;
        let b_tree = b_commit.tree().ok()?;

        // If tree OIDs match, content is identical regardless of history
        if a_tree.id() == b_tree.id() {
            return Some(ContentComparison::Identical);
        }

        let diff = repo
            .diff_tree_to_tree(Some(&a_tree), Some(&b_tree), None)
            .ok()?;
        let stats = diff.stats().ok()?;

        Some(ContentComparison::Different {
            files_changed: stats.files_changed() as u32,
            insertions: stats.insertions() as u32,
            deletions: stats.deletions() as u32,
        })
    })();

    result.unwrap_or(ContentComparison::Incomparable)
}

/// Read squash-base ref to determine squash state.
fn read_squash_state(
    repo: &Repository,
    session_name: &str,
    container_head: &CommitHash,
) -> SquashState {
    let ref_name = format!(
        "refs/claude-container/squash-base/{}",
        session_name
    );

    let squash_base = match repo.find_reference(&ref_name) {
        Ok(reference) => match reference.peel_to_commit() {
            Ok(commit) => CommitHash::new(commit.id().to_string()),
            Err(_) => return SquashState::NoPriorSquash,
        },
        Err(_) => return SquashState::NoPriorSquash,
    };

    // Count commits between squash_base and container_head
    let base_oid = match git2::Oid::from_str(squash_base.as_str()) {
        Ok(o) => o,
        Err(_) => return SquashState::Stale { base: squash_base },
    };
    let head_oid = match git2::Oid::from_str(container_head.as_str()) {
        Ok(o) => o,
        Err(_) => return SquashState::Stale { base: squash_base },
    };

    // Check that squash_base is an ancestor of container_head
    let is_ancestor = repo
        .graph_descendant_of(head_oid, base_oid)
        .unwrap_or(false);

    if !is_ancestor {
        return SquashState::Stale { base: squash_base };
    }

    let new_commits = count_commits_between(repo, base_oid, head_oid).unwrap_or(0);
    if new_commits == 0 {
        SquashState::Stale { base: squash_base }
    } else {
        SquashState::Active {
            base: squash_base,
            new_commits,
        }
    }
}

/// Count commits between `from` (exclusive) and `to` (inclusive).
fn count_commits_between(
    repo: &Repository,
    from: git2::Oid,
    to: git2::Oid,
) -> Result<u32, git2::Error> {
    let mut revwalk = repo.revwalk()?;
    revwalk.push(to)?;
    revwalk.hide(from)?;

    let mut count = 0u32;
    for oid in revwalk {
        let _ = oid?;
        count += 1;
    }
    Ok(count)
}

/// Determine if the target branch is ahead of the session branch,
/// and if so, whether those ahead commits are all squash artifacts
/// (our own squash-merges) or external work by other people.
fn compute_target_ahead(
    repo: &Repository,
    session_name: &str,
    ancestry: &Ancestry,
    squash: &SquashState,
) -> TargetAheadKind {
    // Only relevant when host is ahead of container
    let host_ahead = match ancestry {
        Ancestry::ContainerBehind { host_ahead } => *host_ahead,
        Ancestry::Diverged { host_ahead, .. } => *host_ahead,
        _ => return TargetAheadKind::NotAhead,
    };

    if host_ahead == 0 {
        return TargetAheadKind::NotAhead;
    }

    // If we have an active squash-base, check if the ahead commits
    // are between squash-base and the current target head.
    // Squash-merges create commits on the target with our content but
    // different hashes — these are "our" commits, not external work.
    match squash {
        SquashState::Active { base, .. } => {
            // Look at the target branch (session_name is also used for the session branch).
            // The session branch head is what the container sees.
            // If all commits between session-head and target-head have commit messages
            // matching our squash pattern, they're artifacts.
            //
            // Simple heuristic: if squash-base is recent (active), and the host-ahead
            // count matches what we'd expect from squash-merging, treat as artifacts.
            // For now, check commit messages for the session name pattern.
            let session_branch = format!("refs/heads/{}", session_name);
            let target_ref = repo.find_reference(&session_branch).ok();
            if target_ref.is_none() {
                return TargetAheadKind::HasExternalWork { external_count: host_ahead };
            }

            // Walk the ahead commits and check if they mention the session
            let session_oid = match target_ref.unwrap().peel_to_commit() {
                Ok(c) => c.id(),
                Err(_) => return TargetAheadKind::HasExternalWork { external_count: host_ahead },
            };

            // Find the target branch head (we need to know what "target" is)
            // In the relation, host_head is the target. But we don't have it here.
            // Use the squash-base as lower bound instead.
            let base_oid = match git2::Oid::from_str(base.as_str()) {
                Ok(o) => o,
                Err(_) => return TargetAheadKind::HasExternalWork { external_count: host_ahead },
            };

            // Walk commits from session_oid back to base_oid
            let mut revwalk = match repo.revwalk() {
                Ok(r) => r,
                Err(_) => return TargetAheadKind::HasExternalWork { external_count: host_ahead },
            };
            let _ = revwalk.push(session_oid);
            let _ = revwalk.hide(base_oid);

            let mut external = 0u32;
            for oid_result in revwalk {
                let oid = match oid_result {
                    Ok(o) => o,
                    Err(_) => break,
                };
                if let Ok(commit) = repo.find_commit(oid) {
                    let msg = commit.message().unwrap_or("");
                    // Squash-merge commits typically contain the session name
                    if !msg.contains(session_name) {
                        external += 1;
                    }
                }
            }

            if external == 0 {
                TargetAheadKind::AllSquashArtifacts
            } else {
                TargetAheadKind::HasExternalWork { external_count: external }
            }
        }
        _ => {
            // No squash history — all ahead commits are external
            TargetAheadKind::HasExternalWork { external_count: host_ahead }
        }
    }
}

/// Build a human-readable description of the sync plan.
fn format_plan_description(plan: &SessionSyncPlan) -> String {
    let pulls = plan.pulls().len();
    let pushes = plan.pushes().len();
    let reconciles = plan.reconciles().len();
    let blocked = plan.blocked().len();
    let skipped = plan.skipped().len();
    let total = plan.repo_actions.len();

    let mut parts = Vec::new();
    if pulls > 0 {
        parts.push(format!("{} pull", pulls));
    }
    if pushes > 0 {
        parts.push(format!("{} push", pushes));
    }
    if reconciles > 0 {
        parts.push(format!("{} reconcile", reconciles));
    }
    if blocked > 0 {
        parts.push(format!("{} blocked", blocked));
    }
    if skipped > 0 {
        parts.push(format!("{} skip", skipped));
    }

    if parts.is_empty() {
        format!("Session {}: {} repos, nothing to do", plan.session_name, total)
    } else {
        format!(
            "Session {}: {} repos — {}",
            plan.session_name,
            total,
            parts.join(", ")
        )
    }
}
