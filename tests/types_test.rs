use std::collections::BTreeMap;
use std::path::PathBuf;

use git_sandbox::types::ids::*;
use git_sandbox::types::git::*;
use git_sandbox::types::session::*;
use git_sandbox::types::verified::*;
use git_sandbox::types::config::*;
use git_sandbox::types::image::*;
use git_sandbox::types::docker::*;
use git_sandbox::types::volume::*;

// ============================================================================
// Helpers
// ============================================================================

fn commit(hex: &str) -> CommitHash {
    CommitHash::new(hex)
}

/// 40-char valid hex SHA
fn valid_sha() -> CommitHash {
    commit("abcdef1234567890abcdef1234567890abcdef12")
}

fn another_sha() -> CommitHash {
    commit("1234567890abcdef1234567890abcdef12345678")
}

fn clean(hash: CommitHash) -> GitSide {
    GitSide::Clean { head: hash }
}

fn dirty(hash: CommitHash, files: u32) -> GitSide {
    GitSide::Dirty { head: hash, dirty_files: files }
}

fn pair(name: &str, container: GitSide, host: GitSide, relation: Option<PairRelation>) -> RepoPair {
    RepoPair {
        name: name.to_string(),
        container,
        host,
        relation,
    }
}

fn relation(
    ancestry: Ancestry,
    content: ContentComparison,
    squash: SquashState,
    target_ahead: TargetAheadKind,
) -> Option<PairRelation> {
    Some(PairRelation { ancestry, content, squash, target_ahead })
}

fn dummy_volumes() -> SessionVolumes {
    let name = SessionName::new("test");
    SessionVolumes {
        session: VolumeState::Exists { name: name.session_volume() },
        state: VolumeState::Exists { name: name.state_volume() },
        cargo: VolumeState::Exists { name: name.cargo_volume() },
        npm: VolumeState::Exists { name: name.npm_volume() },
        pip: VolumeState::Exists { name: name.pip_volume() },
    }
}

fn dummy_container_inspect() -> ContainerInspect {
    ContainerInspect {
        image_id: ImageId::new("sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"),
        image_name: ImageRef::new("claude-container:latest"),
        user: "developer".to_string(),
        env_vars: vec![],
        mounts: vec![],
        created: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn binary_check(name: &str, present: bool, functional: bool) -> BinaryCheck {
    BinaryCheck {
        name: name.to_string(),
        present,
        functional,
    }
}

// ============================================================================
// 1. ID types
// ============================================================================

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn session_name_generates_correct_volume_names() {
        let name = SessionName::new("synapse-cc-ux");
        assert_eq!(name.session_volume().as_str(), "claude-session-synapse-cc-ux");
        assert_eq!(name.state_volume().as_str(), "claude-state-synapse-cc-ux");
        assert_eq!(name.cargo_volume().as_str(), "claude-cargo-synapse-cc-ux");
        assert_eq!(name.npm_volume().as_str(), "claude-npm-synapse-cc-ux");
        assert_eq!(name.pip_volume().as_str(), "claude-pip-synapse-cc-ux");
    }

    #[test]
    fn session_name_generates_correct_container_name() {
        let name = SessionName::new("my-session");
        assert_eq!(name.container_name().as_str(), "claude-session-ctr-my-session");
    }

    #[test]
    fn session_name_all_volumes_returns_five() {
        let name = SessionName::new("test");
        assert_eq!(name.all_volumes().len(), 5);
    }

    #[test]
    fn session_name_display() {
        let name = SessionName::new("hello");
        assert_eq!(format!("{}", name), "hello");
    }

    #[test]
    fn commit_hash_is_valid_accepts_hex() {
        assert!(valid_sha().is_valid());
        // 7 chars minimum
        assert!(commit("abcdef1").is_valid());
        // Mixed case hex
        assert!(commit("AbCdEf1234567").is_valid());
    }

    #[test]
    fn commit_hash_is_valid_rejects_ref_names() {
        assert!(!commit("main").is_valid());          // too short + non-hex
        assert!(!commit("HEAD").is_valid());           // non-hex
        assert!(!commit("refs/heads/main").is_valid()); // slashes, non-hex
        assert!(!commit("abc").is_valid());            // too short (< 7)
        assert!(!commit("ghijkl1234567").is_valid());  // non-hex chars g, h, i, j, k, l
    }

    #[test]
    fn commit_hash_is_valid_rejects_short_hex() {
        assert!(!commit("abcdef").is_valid()); // 6 chars, need 7
    }

    #[test]
    fn commit_hash_short_returns_7_chars() {
        let h = valid_sha();
        assert_eq!(h.short().len(), 7);
        assert_eq!(h.short(), "abcdef1");
    }

    #[test]
    fn commit_hash_short_with_short_input() {
        let h = commit("abc");
        assert_eq!(h.short(), "abc"); // min(7, 3) = 3
    }

    #[test]
    fn commit_hash_display_uses_short() {
        let h = valid_sha();
        assert_eq!(format!("{}", h), "abcdef1");
    }

    #[test]
    fn image_id_short_skips_sha256_prefix() {
        let id = ImageId::new("sha256:abcdef1234567890abcdef");
        // skip "sha256:" (7 chars), take 12 chars → "abcdef123456"
        assert_eq!(id.short(), "abcdef123456");
    }

    #[test]
    fn image_id_short_with_short_input() {
        let id = ImageId::new("short");
        assert_eq!(id.short(), "short"); // len <= 19, returns full
    }
}

// ============================================================================
// 2. Git pair sync decision — exhaustive
// ============================================================================

#[cfg(test)]
mod sync_decision_tests {
    use super::*;

    // --- Both Clean, same commit ---

    #[test]
    fn clean_same_clean_same_skips_identical() {
        let sha = valid_sha();
        let p = pair("repo", clean(sha.clone()), clean(sha), None);
        assert_eq!(p.sync_decision(), SyncDecision::Skip { reason: SkipReason::Identical });
    }

    // --- Both Clean, different commits, ContainerAhead ---

    #[test]
    fn clean_clean_container_ahead_pulls() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::ContainerAhead { container_ahead: 3 },
                ContentComparison::Different { files_changed: 2, insertions: 10, deletions: 5 },
                SquashState::NoPriorSquash,
                TargetAheadKind::NotAhead,
            ),
        );
        assert_eq!(p.sync_decision(), SyncDecision::Pull { commits: 3 });
    }

    // --- Both Clean, different commits, ContainerBehind + HasExternalWork ---

    #[test]
    fn clean_clean_container_behind_external_work_pushes() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::ContainerBehind { host_ahead: 2 },
                ContentComparison::Different { files_changed: 1, insertions: 5, deletions: 0 },
                SquashState::NoPriorSquash,
                TargetAheadKind::HasExternalWork { external_count: 2 },
            ),
        );
        assert_eq!(p.sync_decision(), SyncDecision::Push { commits: 2 });
    }

    // --- Both Clean, Diverged ancestry ---

    #[test]
    fn clean_clean_diverged_reconciles() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::Diverged {
                    container_ahead: 4,
                    host_ahead: 2,
                    merge_base: Some(commit("0000000000000000000000000000000000000000")),
                },
                ContentComparison::Different { files_changed: 5, insertions: 20, deletions: 10 },
                SquashState::NoPriorSquash,
                TargetAheadKind::NotAhead,
            ),
        );
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Reconcile { container_ahead: 4, host_ahead: 2 }
        );
    }

    // --- Both Clean, Identical content despite diverged history → SquashIdentical ---

    #[test]
    fn clean_clean_diverged_but_identical_content_skips_squash() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::Diverged {
                    container_ahead: 1,
                    host_ahead: 1,
                    merge_base: None,
                },
                ContentComparison::Identical,
                SquashState::NoPriorSquash,
                TargetAheadKind::NotAhead,
            ),
        );
        assert_eq!(p.sync_decision(), SyncDecision::Skip { reason: SkipReason::SquashIdentical });
    }

    // --- Both Clean, ContainerBehind + AllSquashArtifacts + Identical content → SquashIdentical ---

    #[test]
    fn clean_clean_container_behind_all_squash_identical_content_skips() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::ContainerBehind { host_ahead: 1 },
                ContentComparison::Identical,
                SquashState::NoPriorSquash,
                TargetAheadKind::AllSquashArtifacts,
            ),
        );
        // Content is Identical → early return before ancestry match
        assert_eq!(p.sync_decision(), SyncDecision::Skip { reason: SkipReason::SquashIdentical });
    }

    // --- Both Clean, ContainerBehind + AllSquashArtifacts + Different content → Push ---

    #[test]
    fn clean_clean_container_behind_all_squash_different_content_pushes() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::ContainerBehind { host_ahead: 3 },
                ContentComparison::Different { files_changed: 1, insertions: 1, deletions: 0 },
                SquashState::NoPriorSquash,
                TargetAheadKind::AllSquashArtifacts,
            ),
        );
        assert_eq!(p.sync_decision(), SyncDecision::Push { commits: 3 });
    }

    // --- Both Clean, ContainerBehind + NotAhead → Skip(Identical) ---

    #[test]
    fn clean_clean_container_behind_not_ahead_skips() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::ContainerBehind { host_ahead: 0 },
                ContentComparison::Different { files_changed: 1, insertions: 1, deletions: 0 },
                SquashState::NoPriorSquash,
                TargetAheadKind::NotAhead,
            ),
        );
        assert_eq!(p.sync_decision(), SyncDecision::Skip { reason: SkipReason::Identical });
    }

    // --- Both Clean, Unknown ancestry → Pull ---

    #[test]
    fn clean_clean_unknown_ancestry_pulls() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::Unknown,
                ContentComparison::Different { files_changed: 1, insertions: 1, deletions: 0 },
                SquashState::NoPriorSquash,
                TargetAheadKind::NotAhead,
            ),
        );
        assert_eq!(p.sync_decision(), SyncDecision::Pull { commits: 1 });
    }

    // --- Both Clean, no relation computed → Pull(1) ---

    #[test]
    fn clean_clean_no_relation_pulls_one() {
        let p = pair("repo", clean(valid_sha()), clean(another_sha()), None);
        assert_eq!(p.sync_decision(), SyncDecision::Pull { commits: 1 });
    }

    // --- Both Clean, Same ancestry via relation (not same commit) ---

    #[test]
    fn clean_clean_same_ancestry_in_relation_skips() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::Same,
                ContentComparison::Different { files_changed: 1, insertions: 1, deletions: 0 },
                SquashState::NoPriorSquash,
                TargetAheadKind::NotAhead,
            ),
        );
        // Ancestry::Same in the relation takes the match arm → Skip(Identical)
        // (content is not Identical, so doesn't trigger early return)
        assert_eq!(p.sync_decision(), SyncDecision::Skip { reason: SkipReason::Identical });
    }

    // --- Container Dirty, Host Clean → Blocked(ContainerDirty) ---

    #[test]
    fn dirty_container_clean_host_blocked() {
        let p = pair("repo", dirty(valid_sha(), 5), clean(another_sha()), None);
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Blocked { reason: BlockReason::ContainerDirty(5) }
        );
    }

    // --- Container Merging → Blocked(ContainerMerging) ---

    #[test]
    fn merging_container_clean_host_blocked() {
        let p = pair(
            "repo",
            GitSide::Merging { head: valid_sha() },
            clean(another_sha()),
            None,
        );
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Blocked { reason: BlockReason::ContainerMerging }
        );
    }

    // --- Container Rebasing → Blocked(ContainerRebasing) ---

    #[test]
    fn rebasing_container_clean_host_blocked() {
        let p = pair(
            "repo",
            GitSide::Rebasing { head: valid_sha() },
            clean(another_sha()),
            None,
        );
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Blocked { reason: BlockReason::ContainerRebasing }
        );
    }

    // --- Clean container, Dirty host → Blocked(HostDirty) ---

    #[test]
    fn clean_container_dirty_host_blocked() {
        let p = pair("repo", clean(valid_sha()), dirty(another_sha(), 3), None);
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Blocked { reason: BlockReason::HostDirty }
        );
    }

    // --- Clean container, Merging host → Blocked(HostDirty) ---

    #[test]
    fn clean_container_merging_host_blocked() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            GitSide::Merging { head: another_sha() },
            None,
        );
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Blocked { reason: BlockReason::HostDirty }
        );
    }

    // --- Clean container, Rebasing host → Blocked(HostDirty) ---

    #[test]
    fn clean_container_rebasing_host_blocked() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            GitSide::Rebasing { head: another_sha() },
            None,
        );
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Blocked { reason: BlockReason::HostDirty }
        );
    }

    // --- Missing, Missing → Skip(Identical) ---

    #[test]
    fn missing_missing_skips() {
        let p = pair("repo", GitSide::Missing, GitSide::Missing, None);
        assert_eq!(p.sync_decision(), SyncDecision::Skip { reason: SkipReason::Identical });
    }

    // --- Missing container, Clean host → PushToContainer ---

    #[test]
    fn missing_container_clean_host_push_to_container() {
        let p = pair("repo", GitSide::Missing, clean(another_sha()), None);
        assert_eq!(p.sync_decision(), SyncDecision::PushToContainer);
    }

    // --- Clean container, Missing host → CloneToHost ---

    #[test]
    fn clean_container_missing_host_clone_to_host() {
        let p = pair("repo", clean(valid_sha()), GitSide::Missing, None);
        assert_eq!(p.sync_decision(), SyncDecision::CloneToHost);
    }

    // --- NotARepo container, Clean host → Skip(Identical) ---

    #[test]
    fn not_a_repo_container_clean_host_skips() {
        let p = pair(
            "repo",
            GitSide::NotARepo { path: PathBuf::from("/workspace/repo") },
            clean(another_sha()),
            None,
        );
        assert_eq!(p.sync_decision(), SyncDecision::Skip { reason: SkipReason::Identical });
    }

    // --- Clean container, NotARepo host → Blocked(HostNotARepo) ---

    #[test]
    fn clean_container_not_a_repo_host_blocked() {
        let host_path = PathBuf::from("/home/user/repo");
        let p = pair(
            "repo",
            clean(valid_sha()),
            GitSide::NotARepo { path: host_path.clone() },
            None,
        );
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Blocked { reason: BlockReason::HostNotARepo(host_path) }
        );
    }

    // --- Dirty container takes precedence over dirty host ---

    #[test]
    fn dirty_container_dirty_host_reports_container_dirty() {
        let p = pair("repo", dirty(valid_sha(), 7), dirty(another_sha(), 3), None);
        assert_eq!(
            p.sync_decision(),
            SyncDecision::Blocked { reason: BlockReason::ContainerDirty(7) }
        );
    }

    // --- Missing container, Missing host → Skip ---

    #[test]
    fn missing_container_dirty_host_push_to_container() {
        // Missing matches before Dirty in the match arms
        let p = pair("repo", GitSide::Missing, dirty(another_sha(), 2), None);
        assert_eq!(p.sync_decision(), SyncDecision::PushToContainer);
    }

    // --- ContainerAhead with Identical content still returns SquashIdentical ---

    #[test]
    fn container_ahead_identical_content_skips() {
        let p = pair(
            "repo",
            clean(valid_sha()),
            clean(another_sha()),
            relation(
                Ancestry::ContainerAhead { container_ahead: 5 },
                ContentComparison::Identical,
                SquashState::NoPriorSquash,
                TargetAheadKind::NotAhead,
            ),
        );
        assert_eq!(p.sync_decision(), SyncDecision::Skip { reason: SkipReason::SquashIdentical });
    }
}

// ============================================================================
// 3. Session type-state transitions (compile-time verification)
// ============================================================================

#[cfg(test)]
mod session_typestate_tests {
    use super::*;

    fn default_metadata() -> SessionMetadata {
        SessionMetadata {
            name: SessionName::new("test"),
            ..Default::default()
        }
    }

    /// This test verifies that the full lifecycle compiles.
    /// The type-state pattern means invalid transitions won't compile.
    #[test]
    fn full_lifecycle_uncreated_to_running_to_stopped_to_running() {
        let uncreated = Uncreated { name: SessionName::new("test") };

        // Uncreated → Created
        let created = uncreated.create(default_metadata(), dummy_volumes());
        assert_eq!(created.name.as_str(), "test");

        // Created → Running
        let running = created.start(dummy_container_inspect());
        assert_eq!(running.name.as_str(), "test");

        // Running → Stopped
        let stopped = running.stop();
        assert_eq!(stopped.name.as_str(), "test");

        // Stopped → Running (resume)
        let running_again = stopped.resume();
        assert_eq!(running_again.name.as_str(), "test");
    }

    #[test]
    fn stopped_remove_container_returns_created() {
        let uncreated = Uncreated { name: SessionName::new("test") };
        let created = uncreated.create(default_metadata(), dummy_volumes());
        let running = created.start(dummy_container_inspect());
        let stopped = running.stop();

        // Stopped → Created (remove container)
        let created_again = stopped.remove_container();
        assert_eq!(created_again.name.as_str(), "test");
    }

    #[test]
    fn created_delete_returns_uncreated() {
        let uncreated = Uncreated { name: SessionName::new("test") };
        let created = uncreated.create(default_metadata(), dummy_volumes());

        let uncreated_again = created.delete();
        assert_eq!(uncreated_again.name.as_str(), "test");
    }

    #[test]
    fn stopped_delete_returns_uncreated() {
        let uncreated = Uncreated { name: SessionName::new("test") };
        let created = uncreated.create(default_metadata(), dummy_volumes());
        let running = created.start(dummy_container_inspect());
        let stopped = running.stop();

        let uncreated_again = stopped.delete();
        assert_eq!(uncreated_again.name.as_str(), "test");
    }

    #[test]
    fn metadata_preserves_through_transitions() {
        let meta = SessionMetadata {
            name: SessionName::new("test"),
            enable_docker: true,
            ephemeral: true,
            ..Default::default()
        };
        let uncreated = Uncreated { name: SessionName::new("test") };
        let created = uncreated.create(meta, dummy_volumes());
        assert!(created.metadata.enable_docker);
        assert!(created.metadata.ephemeral);

        let running = created.start(dummy_container_inspect());
        assert!(running.metadata.enable_docker);
        assert!(running.metadata.ephemeral);

        let stopped = running.stop();
        assert!(stopped.metadata.enable_docker);
        assert!(stopped.metadata.ephemeral);
    }
}

// ============================================================================
// 4. Config — main_project selection
// ============================================================================

#[cfg(test)]
mod config_tests {
    use super::*;

    fn project(path: &str, main: bool) -> ProjectConfig {
        ProjectConfig {
            path: PathBuf::from(path),
            extract: true,
            main,
        }
    }

    fn config_with(projects: Vec<(&str, ProjectConfig)>) -> SessionConfig {
        let mut map = BTreeMap::new();
        for (name, cfg) in projects {
            map.insert(name.to_string(), cfg);
        }
        SessionConfig { version: Some("1".to_string()), projects: map }
    }

    #[test]
    fn main_project_explicit_main_true() {
        let config = config_with(vec![
            ("alpha", project("/home/alpha", false)),
            ("beta", project("/home/beta", true)),
            ("gamma", project("/home/gamma", false)),
        ]);
        assert_eq!(config.main_project(None), Some("beta"));
    }

    #[test]
    fn main_project_cwd_match() {
        let config = config_with(vec![
            ("alpha", project("/home/user/alpha", false)),
            ("beta", project("/home/user/beta", false)),
        ]);
        let cwd = PathBuf::from("/home/user/beta/src");
        assert_eq!(config.main_project(Some(&cwd)), Some("beta"));
    }

    #[test]
    fn main_project_cwd_exact_match() {
        let config = config_with(vec![
            ("alpha", project("/home/user/alpha", false)),
            ("beta", project("/home/user/beta", false)),
        ]);
        let cwd = PathBuf::from("/home/user/alpha");
        assert_eq!(config.main_project(Some(&cwd)), Some("alpha"));
    }

    #[test]
    fn main_project_no_match_returns_first() {
        let config = config_with(vec![
            ("alpha", project("/home/user/alpha", false)),
            ("beta", project("/home/user/beta", false)),
        ]);
        let cwd = PathBuf::from("/somewhere/else");
        // BTreeMap is sorted, so "alpha" comes first
        assert_eq!(config.main_project(Some(&cwd)), Some("alpha"));
    }

    #[test]
    fn main_project_no_cwd_no_main_returns_first() {
        let config = config_with(vec![
            ("alpha", project("/home/user/alpha", false)),
            ("beta", project("/home/user/beta", false)),
        ]);
        assert_eq!(config.main_project(None), Some("alpha"));
    }

    #[test]
    fn main_project_empty_projects_returns_none() {
        let config = SessionConfig { version: None, projects: BTreeMap::new() };
        assert_eq!(config.main_project(None), None);
    }

    #[test]
    fn main_project_explicit_main_takes_precedence_over_cwd() {
        let config = config_with(vec![
            ("alpha", project("/home/user/alpha", true)),
            ("beta", project("/home/user/beta", false)),
        ]);
        let cwd = PathBuf::from("/home/user/beta/deep/nested");
        // main:true wins even though cwd matches beta
        assert_eq!(config.main_project(Some(&cwd)), Some("alpha"));
    }
}

// ============================================================================
// 5. Image validation
// ============================================================================

#[cfg(test)]
mod image_validation_tests {
    use super::*;

    fn validation(critical: Vec<BinaryCheck>, optional: Vec<BinaryCheck>) -> ImageValidation {
        ImageValidation {
            image: ImageRef::new("test:latest"),
            critical,
            optional,
        }
    }

    #[test]
    fn is_valid_all_present_and_functional() {
        let v = validation(
            vec![
                binary_check("gosu", true, true),
                binary_check("git", true, true),
                binary_check("claude", true, true),
                binary_check("bash", true, true),
            ],
            vec![
                binary_check("python3", true, true),
            ],
        );
        assert!(v.is_valid());
        assert!(v.missing_critical().is_empty());
    }

    #[test]
    fn is_valid_false_when_critical_missing() {
        let v = validation(
            vec![
                binary_check("gosu", true, true),
                binary_check("git", false, false),
                binary_check("claude", true, true),
                binary_check("bash", true, true),
            ],
            vec![],
        );
        assert!(!v.is_valid());
    }

    #[test]
    fn is_valid_false_when_critical_not_functional() {
        let v = validation(
            vec![
                binary_check("gosu", true, false), // present but not functional
                binary_check("git", true, true),
                binary_check("claude", true, true),
                binary_check("bash", true, true),
            ],
            vec![],
        );
        assert!(!v.is_valid());
    }

    #[test]
    fn missing_critical_returns_correct_names() {
        let v = validation(
            vec![
                binary_check("gosu", true, true),
                binary_check("git", false, false),
                binary_check("claude", true, false),
                binary_check("bash", true, true),
            ],
            vec![],
        );
        let missing = v.missing_critical();
        assert_eq!(missing, vec!["git", "claude"]);
    }

    #[test]
    fn missing_optional_returns_correct_names() {
        let v = validation(
            vec![binary_check("gosu", true, true)],
            vec![
                binary_check("python3", false, false),
                binary_check("sudo", true, true),
                binary_check("docker", false, false),
            ],
        );
        let missing = v.missing_optional();
        assert_eq!(missing, vec!["python3", "docker"]);
    }

    #[test]
    fn is_valid_with_empty_critical_is_valid() {
        let v = validation(vec![], vec![]);
        assert!(v.is_valid());
    }

    #[test]
    fn missing_optional_ignores_functional_flag() {
        // missing_optional only checks `present`, not `functional`
        let v = validation(
            vec![],
            vec![binary_check("python3", true, false)],
        );
        assert!(v.missing_optional().is_empty());
    }
}

// ============================================================================
// 6. Verified wrapper
// ============================================================================

#[cfg(test)]
mod verified_tests {
    use super::*;

    #[test]
    fn new_unchecked_creates_wrapper() {
        let proof = DockerAvailable { version: "24.0.0".to_string() };
        let verified = Verified::__test_new(proof);
        assert_eq!(verified.version, "24.0.0");
    }

    #[test]
    fn deref_gives_access_to_inner() {
        let proof = DockerAvailable { version: "25.0.1".to_string() };
        let verified = Verified::__test_new(proof);
        // Deref lets us access fields directly
        let version: &str = &verified.version;
        assert_eq!(version, "25.0.1");
    }

    #[test]
    fn into_inner_unwraps() {
        let proof = DockerAvailable { version: "24.0.0".to_string() };
        let verified = Verified::__test_new(proof);
        let inner: DockerAvailable = verified.into_inner();
        assert_eq!(inner.version, "24.0.0");
    }

    #[test]
    fn debug_format_shows_verified_prefix() {
        let proof = DockerAvailable { version: "24.0.0".to_string() };
        let verified = Verified::__test_new(proof);
        let dbg = format!("{:?}", verified);
        assert!(dbg.starts_with("Verified("));
    }

    #[test]
    fn clone_works() {
        let proof = DockerAvailable { version: "24.0.0".to_string() };
        let verified = Verified::__test_new(proof);
        let cloned = verified.clone();
        assert_eq!(cloned.version, verified.version);
    }

    #[test]
    fn verified_valid_image() {
        let proof = ValidImage {
            image: ImageRef::new("claude:latest"),
            image_id: ImageId::new("sha256:abc123def456"),
            validation: ImageValidation {
                image: ImageRef::new("claude:latest"),
                critical: vec![binary_check("gosu", true, true)],
                optional: vec![],
            },
        };
        let verified = Verified::__test_new(proof);
        assert_eq!(verified.image.as_str(), "claude:latest");
        assert!(verified.validation.is_valid());
    }
}

// ============================================================================
// Additional: GitSide helper method tests
// ============================================================================

#[cfg(test)]
mod git_side_tests {
    use super::*;

    #[test]
    fn head_returns_some_for_clean() {
        let side = clean(valid_sha());
        assert!(side.head().is_some());
        assert_eq!(side.head().unwrap().short(), "abcdef1");
    }

    #[test]
    fn head_returns_some_for_dirty() {
        let side = dirty(valid_sha(), 3);
        assert!(side.head().is_some());
    }

    #[test]
    fn head_returns_some_for_merging() {
        let side = GitSide::Merging { head: valid_sha() };
        assert!(side.head().is_some());
    }

    #[test]
    fn head_returns_some_for_rebasing() {
        let side = GitSide::Rebasing { head: valid_sha() };
        assert!(side.head().is_some());
    }

    #[test]
    fn head_returns_none_for_missing() {
        assert!(GitSide::Missing.head().is_none());
    }

    #[test]
    fn head_returns_none_for_not_a_repo() {
        let side = GitSide::NotARepo { path: PathBuf::from("/tmp") };
        assert!(side.head().is_none());
    }

    #[test]
    fn is_readable_for_clean_and_dirty() {
        assert!(clean(valid_sha()).is_readable());
        assert!(dirty(valid_sha(), 1).is_readable());
        assert!(!GitSide::Merging { head: valid_sha() }.is_readable());
        assert!(!GitSide::Missing.is_readable());
    }

    #[test]
    fn is_writable_only_for_clean() {
        assert!(clean(valid_sha()).is_writable());
        assert!(!dirty(valid_sha(), 1).is_writable());
        assert!(!GitSide::Missing.is_writable());
    }

    #[test]
    fn is_present_for_repo_states() {
        assert!(clean(valid_sha()).is_present());
        assert!(dirty(valid_sha(), 1).is_present());
        assert!(GitSide::Merging { head: valid_sha() }.is_present());
        assert!(GitSide::Rebasing { head: valid_sha() }.is_present());
        assert!(!GitSide::Missing.is_present());
        assert!(!GitSide::NotARepo { path: PathBuf::from("/tmp") }.is_present());
    }
}
