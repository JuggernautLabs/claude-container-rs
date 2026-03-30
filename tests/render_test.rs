//! Render and display tests — GS-12 UX polish.
//!
//! Tests for human-readable MergeOutcome Display, MergeBlocker Display,
//! and relative path edge cases.

// We test the Display impls by importing the types from the crate
use git_sandbox::types::git::{MergeOutcome, MergeBlocker};
use git_sandbox::types::CommitHash;

// ============================================================================
// MergeOutcome Display — human-readable output
// ============================================================================

#[test]
fn merge_outcome_display_squash_merge() {
    let outcome = MergeOutcome::SquashMerge {
        commits: 3,
        squash_base: CommitHash::new("abc123"),
    };
    assert_eq!(format!("{}", outcome), "squash-merge 3 commit(s)");
}

#[test]
fn merge_outcome_display_fast_forward() {
    let outcome = MergeOutcome::FastForward { commits: 1 };
    assert_eq!(format!("{}", outcome), "fast-forward 1 commit(s)");
}

#[test]
fn merge_outcome_display_fast_forward_multiple() {
    let outcome = MergeOutcome::FastForward { commits: 7 };
    assert_eq!(format!("{}", outcome), "fast-forward 7 commit(s)");
}

#[test]
fn merge_outcome_display_already_up_to_date() {
    let outcome = MergeOutcome::AlreadyUpToDate;
    assert_eq!(format!("{}", outcome), "already up to date");
}

#[test]
fn merge_outcome_display_conflict() {
    let outcome = MergeOutcome::Conflict {
        files: vec!["src/main.rs".into(), "README.md".into()],
    };
    assert_eq!(format!("{}", outcome), "conflict (src/main.rs, README.md)");
}

#[test]
fn merge_outcome_display_clean_merge() {
    let outcome = MergeOutcome::CleanMerge;
    assert_eq!(format!("{}", outcome), "merge cleanly");
}

#[test]
fn merge_outcome_display_create_branch() {
    let outcome = MergeOutcome::CreateBranch {
        from: CommitHash::new("deadbeef"),
    };
    // CommitHash Display uses short() which truncates to 7 chars
    assert_eq!(format!("{}", outcome), "create branch from deadbee");
}

// ============================================================================
// MergeBlocker Display — human-readable blockers
// ============================================================================

#[test]
fn merge_blocker_display_host_dirty() {
    let outcome = MergeOutcome::Blocked(MergeBlocker::HostDirty);
    let s = format!("{}", outcome);
    assert!(s.contains("blocked"), "Should contain 'blocked': {}", s);
    assert!(s.contains("host has uncommitted changes"), "Should describe blocker: {}", s);
}

#[test]
fn merge_blocker_display_no_session_branch() {
    let outcome = MergeOutcome::Blocked(MergeBlocker::NoSessionBranch);
    let s = format!("{}", outcome);
    assert!(s.contains("blocked"), "Should contain 'blocked': {}", s);
    assert!(s.contains("no session branch"), "Should describe blocker: {}", s);
}

#[test]
fn merge_blocker_display_no_target_branch() {
    let outcome = MergeOutcome::Blocked(MergeBlocker::NoTargetBranch);
    let s = format!("{}", outcome);
    assert!(s.contains("blocked"), "Should contain 'blocked': {}", s);
    assert!(s.contains("no target branch"), "Should describe blocker: {}", s);
}

#[test]
fn merge_blocker_display_container_dirty() {
    let outcome = MergeOutcome::Blocked(MergeBlocker::ContainerDirty);
    let s = format!("{}", outcome);
    assert!(s.contains("blocked"), "Should contain 'blocked': {}", s);
    assert!(s.contains("container has uncommitted changes"), "Should describe blocker: {}", s);
}

#[test]
fn merge_blocker_display_merge_in_progress() {
    let outcome = MergeOutcome::Blocked(MergeBlocker::MergeInProgress);
    let s = format!("{}", outcome);
    assert!(s.contains("blocked"), "Should contain 'blocked': {}", s);
    assert!(s.contains("merge in progress"), "Should describe blocker: {}", s);
}

#[test]
fn merge_blocker_display_repo_missing() {
    let outcome = MergeOutcome::Blocked(MergeBlocker::RepoMissing);
    let s = format!("{}", outcome);
    assert!(s.contains("blocked"), "Should contain 'blocked': {}", s);
    assert!(s.contains("repo missing"), "Should describe blocker: {}", s);
}

// ============================================================================
// No {:?} in user-facing Display output
// ============================================================================

#[test]
fn merge_outcome_display_no_debug_format() {
    // All MergeOutcome variants should produce clean human-readable output
    // with no Rust debug formatting artifacts like SquashMerge { commits: 3 }
    let outcomes: Vec<MergeOutcome> = vec![
        MergeOutcome::AlreadyUpToDate,
        MergeOutcome::FastForward { commits: 5 },
        MergeOutcome::SquashMerge { commits: 2, squash_base: CommitHash::new("abc") },
        MergeOutcome::CleanMerge,
        MergeOutcome::Conflict { files: vec!["a.rs".into()] },
        MergeOutcome::CreateBranch { from: CommitHash::new("def") },
        MergeOutcome::Blocked(MergeBlocker::HostDirty),
        MergeOutcome::Blocked(MergeBlocker::NoSessionBranch),
        MergeOutcome::Blocked(MergeBlocker::ContainerDirty),
        MergeOutcome::Blocked(MergeBlocker::MergeInProgress),
        MergeOutcome::Blocked(MergeBlocker::RepoMissing),
    ];

    for outcome in &outcomes {
        let display = format!("{}", outcome);
        // Should not contain Rust Debug artifacts
        assert!(!display.contains("{ "), "Debug format in Display: {}", display);
        assert!(!display.contains(" }"), "Debug format in Display: {}", display);
        // MergeBlocker::Variant pattern should not appear
        assert!(!display.contains("HostDirty"), "Debug variant name in Display: {}", display);
        assert!(!display.contains("NoSessionBranch"), "Debug variant name in Display: {}", display);
        assert!(!display.contains("ContainerDirty"), "Debug variant name in Display: {}", display);
    }
}

// ============================================================================
// Relative path display edge cases
// ============================================================================

#[test]
fn display_name_relative_paths() {
    // Test the pathdiff logic directly (same logic as render::display_name)
    use std::path::PathBuf;

    // cwd=/a/b/c, path=/a/b/d -> ../d
    let rel = pathdiff::diff_paths(
        PathBuf::from("/a/b/d"),
        PathBuf::from("/a/b/c"),
    );
    assert_eq!(rel.unwrap().to_string_lossy(), "../d");

    // cwd=/a/b, path=/a/b/c/d -> c/d
    let rel = pathdiff::diff_paths(
        PathBuf::from("/a/b/c/d"),
        PathBuf::from("/a/b"),
    );
    assert_eq!(rel.unwrap().to_string_lossy(), "c/d");

    // cwd=/x/y/z/w, path=/a/b/c/d -> deeply different, many ../
    let rel = pathdiff::diff_paths(
        PathBuf::from("/a/b/c/d"),
        PathBuf::from("/x/y/z/w"),
    );
    let rel_str = rel.unwrap().to_string_lossy().to_string();
    // Should have more than 3 ../ components, which display_name would reject
    assert!(rel_str.matches("../").count() > 3,
        "Deep path should have many ../: {}", rel_str);

    // cwd=path -> "" which display_name maps to ./reponame
    let rel = pathdiff::diff_paths(
        PathBuf::from("/a/b/c"),
        PathBuf::from("/a/b/c"),
    );
    let s = rel.unwrap().to_string_lossy().to_string();
    assert!(s == "." || s.is_empty(),
        "Same path should produce '.' or '': got '{}'", s);
}
