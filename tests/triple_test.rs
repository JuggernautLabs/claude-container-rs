//! Tests for the Git State Triple: (Container, Session, Target)
//!
//! The triple models three reference points:
//! - Container HEAD: what's in the Docker volume
//! - Session branch HEAD (host): the extraction point on the host
//! - Target branch HEAD (host): where work gets merged (e.g., main)
//!
//! These tests verify that sync_decision() correctly handles all combinations.

use git_sandbox::types::git::*;
use git_sandbox::types::CommitHash;

fn hash(s: &str) -> CommitHash {
    // Pad to 40 chars to look like a real SHA
    CommitHash::new(format!("{:0>40}", s))
}

/// Helper to build a RepoPair with the triple pattern.
fn make_triple(
    container_head: &str,
    session_head: Option<&str>,
    target_head: Option<&str>,
    container_session_ancestry: Ancestry,
    session_target_ancestry: Option<SessionTargetRelation>,
) -> RepoPair {
    let container = GitSide::Clean { head: hash(container_head) };
    let host = match session_head {
        Some(s) => GitSide::Clean { head: hash(s) },
        None => GitSide::Missing,
    };

    let relation = session_head.map(|_| PairRelation {
        ancestry: container_session_ancestry,
        content: ContentComparison::Different {
            files_changed: 1,
            insertions: 10,
            deletions: 5,
        },
        squash: SquashState::NoPriorSquash,
        target_ahead: TargetAheadKind::NotAhead,
    });

    RepoPair {
        name: "test-repo".to_string(),
        container: container,
        host: host,
        relation,
        target_head: target_head.map(|t| hash(t)),
        session_to_target: session_target_ancestry,
    }
}

#[test]
fn triple_container_matches_session_but_session_ahead_of_target() {
    // container=A, session=A, target=B (B is ancestor of A)
    // Decision: MergeToTarget (session->target merge needed)
    let pair = make_triple(
        "aaa", Some("aaa"), Some("bbb"),
        Ancestry::Same,
        Some(SessionTargetRelation {
            ancestry: Ancestry::ContainerAhead { container_ahead: 3 },
            content: ContentComparison::Different {
                files_changed: 2,
                insertions: 20,
                deletions: 10,
            },
        }),
    );

    let decision = pair.sync_decision();
    assert!(
        matches!(decision, SyncDecision::MergeToTarget { session_ahead, .. } if session_ahead == 3),
        "Expected MergeToTarget with session_ahead=3, got {:?}",
        decision,
    );
}

#[test]
fn triple_container_ahead_session_behind_target() {
    // container=C, session=B, target=A (A ancestor of B ancestor of C)
    // Decision: Pull (extract C into session, then merge will follow)
    // The MergeToTarget is implicit: after extraction, session will be ahead of target.
    let pair = make_triple(
        "ccc", Some("bbb"), Some("aaa"),
        Ancestry::ContainerAhead { container_ahead: 2 },
        Some(SessionTargetRelation {
            ancestry: Ancestry::ContainerAhead { container_ahead: 1 },
            content: ContentComparison::Different {
                files_changed: 1,
                insertions: 5,
                deletions: 2,
            },
        }),
    );

    let decision = pair.sync_decision();
    // Primary decision is Pull (extract container work).
    // After pull, session will be even further ahead of target.
    assert!(
        matches!(decision, SyncDecision::Pull { commits: 2 }),
        "Expected Pull {{ commits: 2 }}, got {:?}",
        decision,
    );
}

#[test]
fn triple_all_same() {
    // container=A, session=A, target=A
    // Decision: Skip (truly nothing to do)
    let pair = RepoPair {
        name: "test-repo".to_string(),
        container: GitSide::Clean { head: hash("aaa") },
        host: GitSide::Clean { head: hash("aaa") },
        relation: Some(PairRelation {
            ancestry: Ancestry::Same,
            content: ContentComparison::Identical,
            squash: SquashState::NoPriorSquash,
            target_ahead: TargetAheadKind::NotAhead,
        }),
        target_head: Some(hash("aaa")),
        session_to_target: Some(SessionTargetRelation {
            ancestry: Ancestry::Same,
            content: ContentComparison::Identical,
        }),
    };

    let decision = pair.sync_decision();
    assert!(
        matches!(decision, SyncDecision::Skip { reason: SkipReason::Identical }),
        "Expected Skip(Identical), got {:?}",
        decision,
    );
}

#[test]
fn triple_no_session_branch() {
    // container=A, session=None, target=B
    // Decision: CloneToHost (create session branch)
    let pair = RepoPair {
        name: "test-repo".to_string(),
        container: GitSide::Clean { head: hash("aaa") },
        host: GitSide::Missing,
        relation: None,
        target_head: Some(hash("bbb")),
        session_to_target: None,
    };

    let decision = pair.sync_decision();
    assert!(
        matches!(decision, SyncDecision::CloneToHost),
        "Expected CloneToHost, got {:?}",
        decision,
    );
}

#[test]
fn triple_session_diverged_from_target() {
    // container=A, session=A, target=C (diverged from session)
    // Decision: Reconcile (or MergeToTarget with divergence info)
    // Since container matches session, no extraction needed. But session and target diverged.
    let pair = make_triple(
        "aaa", Some("aaa"), Some("ccc"),
        Ancestry::Same,
        Some(SessionTargetRelation {
            ancestry: Ancestry::Diverged {
                container_ahead: 2,
                host_ahead: 3,
                merge_base: Some(hash("base")),
            },
            content: ContentComparison::Different {
                files_changed: 5,
                insertions: 30,
                deletions: 15,
            },
        }),
    );

    let decision = pair.sync_decision();
    // Session and target diverged, but container matches session.
    // This is a MergeToTarget with diverged ancestry info.
    assert!(
        matches!(decision, SyncDecision::MergeToTarget { .. }),
        "Expected MergeToTarget (diverged), got {:?}",
        decision,
    );
}

#[test]
fn triple_container_ahead_and_session_ahead_of_target() {
    // container=D, session=C, target=A
    // Decision: Pull (extract D->session), then MergeToTarget will be needed
    // The primary decision captures what needs doing NOW (extract).
    let pair = make_triple(
        "ddd", Some("ccc"), Some("aaa"),
        Ancestry::ContainerAhead { container_ahead: 1 },
        Some(SessionTargetRelation {
            ancestry: Ancestry::ContainerAhead { container_ahead: 2 },
            content: ContentComparison::Different {
                files_changed: 3,
                insertions: 25,
                deletions: 8,
            },
        }),
    );

    let decision = pair.sync_decision();
    assert!(
        matches!(decision, SyncDecision::Pull { commits: 1 }),
        "Expected Pull {{ commits: 1 }}, got {:?}",
        decision,
    );
}
