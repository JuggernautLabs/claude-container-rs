//! Tests for GS-2: Typed Error Variants for Sync Operations
//!
//! Verifies that sync errors are exhaustively matchable without string matching.

use gitvm::types::{
    ContainerError,
    action::RepoSyncResult,
    git::MergeOutcome,
};

#[test]
fn merge_conflict_error_carries_file_list() {
    // MergeConflict { repo, files } variant exists and carries data
    let err = ContainerError::MergeConflict {
        repo: "my-repo".to_string(),
        files: vec!["src/main.rs".to_string(), "README.md".to_string()],
    };

    match &err {
        ContainerError::MergeConflict { repo, files } => {
            assert_eq!(repo, "my-repo");
            assert_eq!(files.len(), 2);
            assert_eq!(files[0], "src/main.rs");
            assert_eq!(files[1], "README.md");
        }
        _ => panic!("Expected MergeConflict variant"),
    }

    // Display includes the file list
    let msg = format!("{}", err);
    assert!(msg.contains("my-repo"));
    assert!(msg.contains("src/main.rs"));
}

#[test]
fn collect_conflicts_uses_typed_results_not_strings() {
    // Given a RepoSyncResult::Conflicted, we can extract conflict info
    // without any string matching
    let result = RepoSyncResult::Conflicted {
        repo_name: "my-repo".to_string(),
        files: vec!["lib.rs".to_string(), "mod.rs".to_string()],
    };

    match &result {
        RepoSyncResult::Conflicted { repo_name, files } => {
            assert_eq!(repo_name, "my-repo");
            assert_eq!(files, &["lib.rs", "mod.rs"]);
        }
        _ => panic!("Expected Conflicted variant"),
    }

    // Also: Pulled with MergeOutcome::Conflict carries files
    let pulled_with_conflict = RepoSyncResult::Pulled {
        repo_name: "other-repo".to_string(),
        extract: gitvm::types::action::ExtractResult {
            commit_count: 5,
            new_head: gitvm::types::CommitHash::new("abc123".to_string()),
        },
        merge: MergeOutcome::Conflict {
            files: vec!["conflict.rs".to_string()],
        },
    };

    // We can match on the merge outcome without string inspection
    if let RepoSyncResult::Pulled { merge: MergeOutcome::Conflict { files }, .. } = &pulled_with_conflict {
        assert_eq!(files, &["conflict.rs"]);
    } else {
        panic!("Expected Pulled with Conflict merge outcome");
    }
}

#[test]
fn extraction_failed_distinguishes_bundle_vs_fetch() {
    // BundleFailed, FetchFailed, and BranchCreateFailed are distinct variants
    let bundle_err = ContainerError::BundleFailed {
        repo: "repo-a".to_string(),
        reason: "git bundle exited 128".to_string(),
    };
    let fetch_err = ContainerError::FetchFailed {
        repo: "repo-b".to_string(),
        reason: "git fetch failed".to_string(),
    };
    let branch_err = ContainerError::BranchCreateFailed {
        repo: "repo-c".to_string(),
        reason: "FETCH_HEAD not set".to_string(),
    };

    // Each is a distinct variant — exhaustive matching works
    match &bundle_err {
        ContainerError::BundleFailed { repo, reason } => {
            assert_eq!(repo, "repo-a");
            assert!(reason.contains("bundle"));
        }
        _ => panic!("Expected BundleFailed"),
    }

    match &fetch_err {
        ContainerError::FetchFailed { repo, reason } => {
            assert_eq!(repo, "repo-b");
            assert!(reason.contains("fetch"));
        }
        _ => panic!("Expected FetchFailed"),
    }

    match &branch_err {
        ContainerError::BranchCreateFailed { repo, reason } => {
            assert_eq!(repo, "repo-c");
            assert!(reason.contains("FETCH_HEAD"));
        }
        _ => panic!("Expected BranchCreateFailed"),
    }
}

#[test]
fn inject_failed_is_distinct_from_extraction() {
    // InjectionFailed { repo, reason } is its own variant, not overloaded on ExtractionFailed
    let inject_err = ContainerError::InjectionFailed {
        repo: "my-repo".to_string(),
        reason: "inject (git fetch+merge) exited with code 1".to_string(),
    };

    match &inject_err {
        ContainerError::InjectionFailed { repo, reason } => {
            assert_eq!(repo, "my-repo");
            assert!(reason.contains("inject"));
        }
        _ => panic!("Expected InjectionFailed"),
    }

    // Make sure it's distinct from BundleFailed
    assert!(!matches!(inject_err, ContainerError::BundleFailed { .. }));
}

#[test]
fn sync_result_conflicted_count() {
    // SyncResult can count conflicted repos
    use gitvm::types::action::SyncResult;
    use gitvm::types::SessionName;

    let result = SyncResult {
        session_name: SessionName::new("test-session".to_string()),
        results: vec![
            RepoSyncResult::Pushed { repo_name: "ok-repo".to_string() },
            RepoSyncResult::Conflicted {
                repo_name: "bad-repo".to_string(),
                files: vec!["a.rs".to_string()],
            },
            RepoSyncResult::Conflicted {
                repo_name: "worse-repo".to_string(),
                files: vec!["b.rs".to_string(), "c.rs".to_string()],
            },
        ],
    };

    // We can collect conflicts by pattern matching, no string inspection needed
    let conflicts: Vec<_> = result.results.iter().filter_map(|r| {
        match r {
            RepoSyncResult::Conflicted { repo_name, files } => Some((repo_name.clone(), files.clone())),
            _ => None,
        }
    }).collect();

    assert_eq!(conflicts.len(), 2);
    assert_eq!(conflicts[0].0, "bad-repo");
    assert_eq!(conflicts[1].0, "worse-repo");
}
