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
    SessionSyncPlan, SquashState, SyncDecision, SyncResult, TargetAheadKind, VolumeRepo,
};

/// Git utility image used for container-side scans.
const GIT_UTIL_IMAGE: &str = "alpine/git";

fn rand_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{:x}", t.as_nanos() % 0xFFFFFF)
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
    pub fn classify_repo(
        &self,
        repo_name: &str,
        container: &VolumeRepo,
        host_path: &Path,
        session_name: &str,
        _target_branch: &str,
    ) -> RepoPair {
        // Container side
        let container_side = volume_repo_to_gitside(container);

        // Host side — open repo via git2
        let (host_side, relation) = match Repository::open(host_path) {
            Ok(repo) => {
                let host_side = read_host_side(&repo, session_name);
                let relation = match (container_side.head(), host_side.head()) {
                    (Some(c_head), Some(h_head)) => {
                        Some(self.compute_relation(&repo, c_head, h_head, session_name))
                    }
                    _ => None,
                };
                (host_side, relation)
            }
            Err(_) => {
                if host_path.exists() {
                    (
                        GitSide::NotARepo {
                            path: host_path.to_path_buf(),
                        },
                        None,
                    )
                } else {
                    (GitSide::Missing, None)
                }
            }
        };

        RepoPair {
            name: repo_name.to_string(),
            container: container_side,
            host: host_side,
            relation,
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

            let decision = pair.sync_decision();

            // Step 3: compute diffs for repos that need them
            let (outbound_diff, inbound_diff) = match &decision {
                SyncDecision::Pull { .. } => {
                    let diff = pair
                        .host
                        .head()
                        .and_then(|h_head| {
                            pair.container
                                .head()
                                .and_then(|c_head| self.compute_diff(&host_path, h_head, c_head))
                        });
                    (diff, None)
                }
                SyncDecision::Push { .. } => {
                    let diff = pair
                        .container
                        .head()
                        .and_then(|c_head| {
                            pair.host
                                .head()
                                .and_then(|h_head| self.compute_diff(&host_path, c_head, h_head))
                        });
                    (None, diff)
                }
                SyncDecision::Reconcile { .. } => {
                    let outbound = pair.host.head().and_then(|h_head| {
                        pair.relation.as_ref().and_then(|rel| match &rel.ancestry {
                            Ancestry::Diverged { merge_base, .. } => merge_base
                                .as_ref()
                                .and_then(|mb| self.compute_diff(&host_path, mb, h_head)),
                            _ => None,
                        })
                    });
                    let inbound = pair.container.head().and_then(|c_head| {
                        pair.relation.as_ref().and_then(|rel| match &rel.ancestry {
                            Ancestry::Diverged { merge_base, .. } => merge_base
                                .as_ref()
                                .and_then(|mb| self.compute_diff(&host_path, mb, c_head)),
                            _ => None,
                        })
                    });
                    (outbound, inbound)
                }
                _ => (None, None),
            };

            repo_actions.push(RepoSyncAction {
                repo_name: vr.name.clone(),
                decision,
                outbound_diff,
                inbound_diff,
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

        Some(DiffSummary {
            files_changed: stats.files_changed() as u32,
            insertions: stats.insertions() as u32,
            deletions: stats.deletions() as u32,
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
        let target_ahead = TargetAheadKind::NotAhead; // TODO: full target-ahead analysis

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

        // Create a temp dir on the host to receive the bundle
        let bundle_dir = tempfile::tempdir().map_err(|e| ContainerError::Io(e))?;
        let bundle_host_path = bundle_dir.path().join("repo.bundle");

        let container_name = format!("cc-extract-{}-{}", session, rand_suffix());
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        // Script: cd into the repo inside /session, create bundle
        let script = format!(
            r#"
git config --global --add safe.directory "*"
cd "/session/{repo_name}" || exit 1
git bundle create /bundles/repo.bundle HEAD 2>&1
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
                Err(e) => {
                    // Clean up and propagate
                    let _ = self.docker.remove_container(
                        &container_name,
                        Some(RemoveContainerOptions { force: true, ..Default::default() }),
                    ).await;
                    return Err(ContainerError::Docker(e));
                }
            }
        }

        // Clean up the container
        let _ = self.docker.remove_container(
            &container_name,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        ).await;

        if exit_code != 0 {
            return Err(ContainerError::ExtractionFailed {
                repo: repo_name.to_string(),
                reason: format!("git bundle create exited with code {}", exit_code),
            });
        }

        // Verify the bundle file exists
        if !bundle_host_path.exists() {
            return Err(ContainerError::ExtractionFailed {
                repo: repo_name.to_string(),
                reason: "bundle file was not created".to_string(),
            });
        }

        // On the host: open the repo and fetch from the bundle
        let repo = Repository::open(host_path).map_err(|_| ContainerError::NotAGitRepo(host_path.to_path_buf()))?;

        let bundle_path_str = bundle_host_path.to_string_lossy().to_string();

        // Add a temporary remote pointing at the bundle file
        let remote_name = format!("_cc_bundle_{}", rand_suffix());
        let mut remote = repo.remote(&remote_name, &bundle_path_str)?;

        // Fetch from the bundle
        remote.fetch(&["HEAD"], None, None)?;
        drop(remote);

        // Resolve FETCH_HEAD
        let fetch_head = repo.find_reference("FETCH_HEAD")?;
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

        // Clean up the temporary remote
        repo.remote_delete(&remote_name).ok();

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
                        None // stale squash-base, ignore it
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

                // Clean up the merge state
                repo.cleanup_state()?;

                return Ok(MergeOutcome::Conflict { files: conflict_files });
            }

            // Write the tree from the merged index
            let mut index = repo.index()?;
            let tree_id = index.write_tree()?;
            let tree = repo.find_tree(tree_id)?;

            // Create a squash commit (single parent = target)
            let sig = repo.signature().unwrap_or_else(|_| {
                git2::Signature::now("claude-container", "cc@localhost").unwrap()
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
            return Ok(MergeOutcome::Conflict { files: conflict_files });
        }

        // Commit the merge
        let mut index = repo.index()?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;

        let sig = repo.signature().unwrap_or_else(|_| {
            git2::Signature::now("claude-container", "cc@localhost").unwrap()
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
            return Err(ContainerError::ExtractionFailed {
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
    // Execute: orchestrate a full sync plan
    // ========================================================================

    /// Execute a sync plan: for each repo, perform extract/merge/inject
    /// according to the decision.
    pub async fn execute_sync(
        &self,
        session: &SessionName,
        plan: SessionSyncPlan,
        repo_configs: &BTreeMap<String, PathBuf>,
    ) -> Result<SyncResult, ContainerError> {
        let target_branch = plan.target_branch.clone();
        let mut results = Vec::new();

        for action in &plan.repo_actions {
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

            let session_branch = format!("cc/{}/{}", session, action.repo_name);

            let result = match &action.decision {
                SyncDecision::Skip { reason } => {
                    RepoSyncResult::Skipped {
                        repo_name: action.repo_name.clone(),
                        reason: format!("{:?}", reason),
                    }
                }

                SyncDecision::Pull { .. } => {
                    match self.execute_pull(
                        session,
                        &action.repo_name,
                        &host_path,
                        &session_branch,
                        &target_branch,
                    ).await {
                        Ok(r) => r,
                        Err(e) => RepoSyncResult::Failed {
                            repo_name: action.repo_name.clone(),
                            error: e.to_string(),
                        },
                    }
                }

                SyncDecision::Push { .. } => {
                    match self.inject(
                        session,
                        &action.repo_name,
                        &host_path,
                        &target_branch,
                    ).await {
                        Ok(()) => RepoSyncResult::Pushed {
                            repo_name: action.repo_name.clone(),
                        },
                        Err(e) => RepoSyncResult::Failed {
                            repo_name: action.repo_name.clone(),
                            error: e.to_string(),
                        },
                    }
                }

                SyncDecision::Reconcile { .. } => {
                    // Reconcile: inject host → container, then extract + merge back
                    let inject_result = self.inject(
                        session,
                        &action.repo_name,
                        &host_path,
                        &target_branch,
                    ).await;

                    match inject_result {
                        Ok(()) => {
                            match self.execute_pull(
                                session,
                                &action.repo_name,
                                &host_path,
                                &session_branch,
                                &target_branch,
                            ).await {
                                Ok(r) => r,
                                Err(e) => RepoSyncResult::Failed {
                                    repo_name: action.repo_name.clone(),
                                    error: format!("reconcile pull phase failed: {}", e),
                                },
                            }
                        }
                        Err(e) => RepoSyncResult::Failed {
                            repo_name: action.repo_name.clone(),
                            error: format!("reconcile inject phase failed: {}", e),
                        },
                    }
                }

                SyncDecision::CloneToHost => {
                    match self.extract(
                        session,
                        &action.repo_name,
                        &host_path,
                        &session_branch,
                    ).await {
                        Ok(extract) => RepoSyncResult::ClonedToHost {
                            repo_name: action.repo_name.clone(),
                            extract,
                        },
                        Err(e) => RepoSyncResult::Failed {
                            repo_name: action.repo_name.clone(),
                            error: e.to_string(),
                        },
                    }
                }

                SyncDecision::PushToContainer => {
                    match self.inject(
                        session,
                        &action.repo_name,
                        &host_path,
                        &target_branch,
                    ).await {
                        Ok(()) => RepoSyncResult::Pushed {
                            repo_name: action.repo_name.clone(),
                        },
                        Err(e) => RepoSyncResult::Failed {
                            repo_name: action.repo_name.clone(),
                            error: e.to_string(),
                        },
                    }
                }

                SyncDecision::Blocked { reason } => {
                    RepoSyncResult::Skipped {
                        repo_name: action.repo_name.clone(),
                        reason: format!("blocked: {:?}", reason),
                    }
                }
            };

            results.push(result);
        }

        Ok(SyncResult {
            session_name: session.clone(),
            results,
        })
    }

    // ========================================================================
    // Internal: execute a pull (extract + merge) for one repo
    // ========================================================================

    async fn execute_pull(
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
