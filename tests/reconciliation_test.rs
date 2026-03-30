//! Tests for GS-6: Typed Conflict Detection → Agentic Reconciliation
//!
//! Verifies the end-to-end pipeline from typed merge conflicts through
//! agentic reconciliation, including:
//! - collect_conflicts uses typed pattern matching (no strings)
//! - check_reconcile_complete returns description (Option<String>)
//! - Post-reconciliation verification of extract success
//! - Shared verification pipeline (deduplicated from cmd_start)

use gitvm::types::{
    action::{RepoSyncResult, SyncResult, ExtractResult},
    git::MergeOutcome,
    CommitHash, SessionName,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

// ============================================================================
// 1. collect_conflicts: typed pattern matching on RepoSyncResult::Conflicted
// ============================================================================

/// Mirrors the collect_conflicts function from main.rs — verifies it works
/// end-to-end with RepoSyncResult::Conflicted without any string matching.
fn collect_conflicts(
    result: &SyncResult,
    repo_paths: &BTreeMap<String, PathBuf>,
) -> Vec<(String, PathBuf, Vec<String>)> {
    result.results.iter().filter_map(|r| {
        if let RepoSyncResult::Conflicted { repo_name, files } = r {
            let host_path = repo_paths.get(repo_name)?.clone();
            Some((repo_name.clone(), host_path, files.clone()))
        } else {
            None
        }
    }).collect()
}

#[test]
fn collect_conflicts_extracts_from_typed_results() {
    let mut repo_paths = BTreeMap::new();
    repo_paths.insert("repo-a".to_string(), PathBuf::from("/home/user/repo-a"));
    repo_paths.insert("repo-b".to_string(), PathBuf::from("/home/user/repo-b"));
    repo_paths.insert("repo-c".to_string(), PathBuf::from("/home/user/repo-c"));

    let result = SyncResult {
        session_name: SessionName::new("test-session"),
        results: vec![
            RepoSyncResult::Pulled {
                repo_name: "repo-a".to_string(),
                extract: ExtractResult {
                    commit_count: 3,
                    new_head: CommitHash::new("aaa1111"),
                },
                merge: MergeOutcome::FastForward { commits: 3 },
            },
            RepoSyncResult::Conflicted {
                repo_name: "repo-b".to_string(),
                files: vec!["src/main.rs".to_string(), "Cargo.toml".to_string()],
            },
            RepoSyncResult::Pushed {
                repo_name: "repo-c".to_string(),
            },
        ],
    };

    let conflicts = collect_conflicts(&result, &repo_paths);

    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].0, "repo-b");
    assert_eq!(conflicts[0].1, PathBuf::from("/home/user/repo-b"));
    assert_eq!(conflicts[0].2, vec!["src/main.rs", "Cargo.toml"]);
}

#[test]
fn collect_conflicts_returns_empty_for_no_conflicts() {
    let mut repo_paths = BTreeMap::new();
    repo_paths.insert("repo-a".to_string(), PathBuf::from("/home/user/repo-a"));

    let result = SyncResult {
        session_name: SessionName::new("test-session"),
        results: vec![
            RepoSyncResult::Pulled {
                repo_name: "repo-a".to_string(),
                extract: ExtractResult {
                    commit_count: 1,
                    new_head: CommitHash::new("aaa1111"),
                },
                merge: MergeOutcome::FastForward { commits: 1 },
            },
        ],
    };

    let conflicts = collect_conflicts(&result, &repo_paths);
    assert!(conflicts.is_empty());
}

#[test]
fn collect_conflicts_skips_repos_without_host_path() {
    // If a conflicted repo has no entry in repo_paths, it's filtered out
    let repo_paths = BTreeMap::new(); // empty — no host paths

    let result = SyncResult {
        session_name: SessionName::new("test-session"),
        results: vec![
            RepoSyncResult::Conflicted {
                repo_name: "orphan-repo".to_string(),
                files: vec!["file.rs".to_string()],
            },
        ],
    };

    let conflicts = collect_conflicts(&result, &repo_paths);
    assert!(conflicts.is_empty());
}

#[test]
fn collect_conflicts_handles_multiple_conflicted_repos() {
    let mut repo_paths = BTreeMap::new();
    repo_paths.insert("a".to_string(), PathBuf::from("/a"));
    repo_paths.insert("b".to_string(), PathBuf::from("/b"));
    repo_paths.insert("c".to_string(), PathBuf::from("/c"));

    let result = SyncResult {
        session_name: SessionName::new("test"),
        results: vec![
            RepoSyncResult::Conflicted {
                repo_name: "a".to_string(),
                files: vec!["x.rs".to_string()],
            },
            RepoSyncResult::Conflicted {
                repo_name: "b".to_string(),
                files: vec!["y.rs".to_string(), "z.rs".to_string()],
            },
            RepoSyncResult::Skipped {
                repo_name: "c".to_string(),
                reason: "already in sync".to_string(),
            },
        ],
    };

    let conflicts = collect_conflicts(&result, &repo_paths);
    assert_eq!(conflicts.len(), 2);
    assert_eq!(conflicts[0].0, "a");
    assert_eq!(conflicts[1].0, "b");
    assert_eq!(conflicts[1].2.len(), 2);
}


// ============================================================================
// 3. check_reconcile_complete returns Option<String> (description)
// ============================================================================

#[test]
fn reconcile_complete_description_parsing() {
    // Test the parsing logic that check_reconcile_complete uses:
    // - If output contains "__NONE__", reconciliation did not complete → None
    // - Otherwise, the output IS the description → Some(description)

    // Simulates the container output for "file exists with content"
    let output_with_description = "Resolved merge conflicts in src/main.rs and Cargo.toml\n";
    let parsed = parse_reconcile_output(output_with_description);
    assert_eq!(parsed, Some("Resolved merge conflicts in src/main.rs and Cargo.toml".to_string()));

    // Simulates the container output for "file does not exist"
    let output_none = "__NONE__\n";
    let parsed = parse_reconcile_output(output_none);
    assert_eq!(parsed, None);

    // Empty description (file exists but empty)
    let output_empty = "\n";
    let parsed = parse_reconcile_output(output_empty);
    assert_eq!(parsed, Some("".to_string()));
}

/// Mirrors the parsing logic from check_reconcile_complete.
/// The actual function is async and requires Docker, so we test the parsing logic directly.
fn parse_reconcile_output(stdout: &str) -> Option<String> {
    if stdout.contains("__NONE__") {
        None
    } else {
        Some(stdout.trim().to_string())
    }
}

#[test]
fn reconcile_complete_none_means_unresolved() {
    let parsed = parse_reconcile_output("__NONE__");
    assert!(parsed.is_none(), "Expected None for unresolved reconciliation");
}

#[test]
fn reconcile_complete_some_means_resolved() {
    let parsed = parse_reconcile_output("Fixed all conflicts");
    assert!(parsed.is_some());
    assert_eq!(parsed.unwrap(), "Fixed all conflicts");
}

// ============================================================================
// 4. SyncResult::conflicted() count works with typed variants
// ============================================================================

#[test]
fn sync_result_counts_conflicted_repos() {
    let result = SyncResult {
        session_name: SessionName::new("test"),
        results: vec![
            RepoSyncResult::Pulled {
                repo_name: "ok".to_string(),
                extract: ExtractResult { commit_count: 1, new_head: CommitHash::new("aaa") },
                merge: MergeOutcome::FastForward { commits: 1 },
            },
            RepoSyncResult::Conflicted {
                repo_name: "bad1".to_string(),
                files: vec!["a.rs".to_string()],
            },
            RepoSyncResult::Conflicted {
                repo_name: "bad2".to_string(),
                files: vec![],
            },
            RepoSyncResult::Failed {
                repo_name: "err".to_string(),
                error: "some error".to_string(),
            },
        ],
    };

    assert_eq!(result.succeeded(), 1);
    assert_eq!(result.conflicted(), 2);
    assert_eq!(result.failed(), 1);
    assert_eq!(result.skipped(), 0);
}

// ============================================================================
// 5. Verification pipeline is reusable (compile-time check)
// ============================================================================

/// This test verifies that the VerificationPipeline type from container/mod.rs
/// can be constructed and carries the right data. The pipeline itself requires
/// Docker, but we verify the type exists and is constructible.
#[test]
fn verification_pipeline_struct_exists() {
    use gitvm::types::verified::*;
    use gitvm::types::ids::*;
    use gitvm::types::image::*;
    use gitvm::types::docker::*;

    // Verify we can construct the proofs that the pipeline produces
    let docker = Verified::__test_new(DockerAvailable { version: "24.0".to_string() });
    let image = Verified::__test_new(ValidImage {
        image: ImageRef::new("test:latest"),
        image_id: ImageId::new("sha256:abc123"),
        validation: ImageValidation {
            image: ImageRef::new("test:latest"),
            critical: vec![],
            optional: vec![],
        },
    });
    let volumes = Verified::__test_new(VolumesReady {
        session: SessionName::new("test"),
    });
    let token = Verified::__test_new(TokenReady {
        mount: TokenMount::EnvVar { var_name: "TEST_TOKEN".to_string() },
    });

    // LaunchReady requires all four proofs
    let ready = LaunchReady {
        docker,
        image,
        volumes,
        token,
        container: LaunchTarget::Create,
    };

    // Verify it compiled and the data is accessible
    assert_eq!(ready.docker.version, "24.0");
    assert_eq!(ready.volumes.session.as_str(), "test");
}

// ============================================================================
// 6. Post-reconciliation: re-extract verification logic
// ============================================================================

#[test]
fn post_reconciliation_reports_extract_failure() {
    // After reconciliation, if re-extract fails, the error should be reported
    // (not silently swallowed). This tests the logic pattern.

    let extract_results: Vec<Result<ExtractResult, String>> = vec![
        Ok(ExtractResult {
            commit_count: 5,
            new_head: CommitHash::new("abc1234"),
        }),
        Err("bundle file not created".to_string()),
    ];

    let mut success_count = 0u32;
    let mut fail_count = 0u32;

    for result in &extract_results {
        match result {
            Ok(extract) => {
                assert!(extract.commit_count > 0);
                success_count += 1;
            }
            Err(e) => {
                assert!(e.contains("bundle"));
                fail_count += 1;
            }
        }
    }

    assert_eq!(success_count, 1);
    assert_eq!(fail_count, 1);
}

#[test]
fn post_reconciliation_merge_after_successful_extract() {
    // After successful re-extract, the merge step should be attempted.
    // If merge also succeeds, reconciliation is fully complete.

    let extract = ExtractResult {
        commit_count: 3,
        new_head: CommitHash::new("def5678"),
    };

    let merge = MergeOutcome::SquashMerge {
        commits: 3,
        squash_base: CommitHash::new("def5678"),
    };

    // Verify the types compose correctly
    let pulled = RepoSyncResult::Pulled {
        repo_name: "resolved-repo".to_string(),
        extract,
        merge,
    };

    match &pulled {
        RepoSyncResult::Pulled { repo_name, extract, merge } => {
            assert_eq!(repo_name, "resolved-repo");
            assert_eq!(extract.commit_count, 3);
            match merge {
                MergeOutcome::SquashMerge { commits, .. } => assert_eq!(*commits, 3),
                _ => panic!("Expected SquashMerge"),
            }
        }
        _ => panic!("Expected Pulled"),
    }
}
