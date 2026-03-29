# VM-1: Sync Virtual Machine — Implementation Plan

## Starting point

Layer 1 (two-leg state model) is shipped. `RepoState`, `PullAction`,
`PushAction` drive dispatch and rendering. 206 tests pass. Dead
`SyncDecision` code remains. Three cleanup bugs (inject, merge, clone)
known and unfixed.

## End state

All sync execution flows through a VM that operates at the primitive
level. 12 irreducible ops, no compounds. High-level commands (push,
pull, sync) generate programs as `Vec<Op>`. The VM validates
preconditions, dispatches to a backend, updates state from
postconditions. Container lifecycle is first-class — entering,
attaching, running throwaway containers are ops like any other.

## The 12 Primitives

### Ref ops — read/write git refs on either side

```
ref_read(side, repo)             → CommitHash
ref_write(side, repo, hash)      → ()
```

`side` is Container or Host. These are the only ops that touch git
refs. Everything else is content manipulation or transport.

### Tree ops — manipulate git content

```
tree_compare(repo, a, b)         → ContentComparison
ancestry_check(repo, a, b)       → Ancestry
merge_trees(repo, ours, theirs)  → MergeResult { tree | conflicts }
checkout(host, repo, ref)        → ()
commit(host, repo, tree, msg)    → CommitHash
```

`merge_trees` is pure — in-memory, no side effects. `checkout` and
`commit` modify the host worktree/index. These never touch the
container.

### Transport ops — move data between host and container

```
bundle_create(container, repo)   → BundlePath
bundle_fetch(host, repo, path)   → CommitHash
```

These are the bridge. `bundle_create` runs inside a throwaway
container. `bundle_fetch` runs on the host. The bundle is the unit
of transport.

### Container ops — lifecycle management

```
run_container(image, script, mounts)  → ExitCode + Output
attach_container(image, env, mounts)  → ExitCode
```

`run_container` is throwaway — run a script, collect output, remove.
Used by inject (fetch+merge script), clone (git clone script),
snapshot (scan script), merge_into_volume (merge-with-markers script).

`attach_container` is interactive — TTY attached, agent runs inside.
Used by reconciliation (Claude resolves conflicts), normal sessions
(Claude works), headless runs (Claude -p).

These are the only ops that talk to Docker.

### Control ops

```
prompt_user(message)              → bool
```

User Y/N gate. Pure control flow.

## The VM State

```rust
struct SyncVM {
    repos: BTreeMap<String, RepoVM>,
    session_name: SessionName,
    target_branch: String,
    backend: Box<dyn Backend>,
    trace: Vec<(Op, OpResult)>,
}

struct RepoVM {
    // Three refs
    container_head: RefState,    // At(hash) | Absent
    session_head: RefState,
    target_head: RefState,

    // Worktree/index state
    container_clean: bool,
    host_clean: bool,
    host_merge_state: MergeState,  // Clean | Merging | Conflicted

    // Conflict state (for agent resolution)
    conflict: ConflictState,     // Clean | Markers(files) | Resolved

    // Cached observations
    extraction_leg: Option<LegState>,
    merge_leg: Option<MergeLeg>,
}
```

Each primitive checks preconditions against this state, executes via
backend, and updates the state from the result.

## How Compounds Decompose

The current 8 compound ops become programs over the 12 primitives.
The VM never sees "extract" — it sees the 3 primitives that extract
is made of.

### observe (per repo)

```
run_container(alpine/git, scan_script, [session_vol:ro])  → output
parse output → for each repo:
    ref_read(container, repo)
    ref_read(host_session, repo)
    ref_read(host_target, repo)
    ancestry_check(repo, container, session)
    ancestry_check(repo, session, target)
    tree_compare(repo, container, session)
    tree_compare(repo, session, target)
→ populate RepoVM state
```

### extract(repo)

```
bundle_create(container, repo)                → bundle_path
bundle_fetch(host, repo, bundle_path)         → fetched_hash
ref_write(host_session, repo, fetched_hash)   → ()
```

### inject(repo, branch)

```
run_container(alpine/git, [
    "git remote add _cc_upstream /upstream &&
     git fetch _cc_upstream {branch} &&
     git merge _cc_upstream/{branch} --no-edit &&
     git remote remove _cc_upstream"
], [host_repo:ro, session_vol:rw])            → exit_code
```

Or decomposed further:
```
ref_read(host_target, repo)                   → target_hash
bundle_create(host, repo)                     → bundle (target branch)
bundle_fetch(container, repo, bundle)         → fetched_hash
merge_trees(container, container_head, fetched_hash) → result
if result.clean:
    commit(container, repo, result.tree, msg) → new_hash
    ref_write(container, repo, new_hash)
```

(Inject currently uses `run_container` with a script. The decomposed
form would eliminate the throwaway container for inject, doing it
through bundles in both directions. That's a future optimization.)

### merge(repo, from, to, squash)

```
ref_read(host, from)                          → source_hash
ref_read(host, to)                            → target_hash
ancestry_check(repo, source, target)          → ancestry
checkout(host, repo, to)
merge_trees(repo, target_hash, source_hash)   → result
if result.conflicted:
    checkout(host, repo, to)                  ← rollback
    return Conflict(files)
commit(host, repo, result.tree, msg)          → new_hash
ref_write(host_target, repo, new_hash)
if squash:
    ref_write(host_squash_base, repo, source_hash)
```

### clone_into_volume(repo)

```
run_container(alpine/git, [
    "git clone /upstream /workspace/{repo} &&
     chown -R {uid}:{gid} /workspace/{repo}"
], [host_repo:ro, session_vol:rw])            → exit_code
```

### merge_into_volume(repo, branch)

```
run_container(alpine/git, [
    "git fetch /upstream {branch} &&
     git merge FETCH_HEAD --no-commit"
], [host_repo:ro, session_vol:rw])            → exit_code + conflict_files
```

### launch_reconciliation(repo)

```
attach_container(claude_image, {
    AGENT_TASK: resolve-conflicts,
    AGENT_CONTEXT: conflict_summary,
    ...
}, [session_vol:rw, state_vol:rw])            → exit_code
```

### normal session (start)

```
attach_container(claude_image, {
    AGENT_TASK: work,
    ...
}, [session_vol:rw, state_vol:rw, host_repos:ro])  → exit_code
```

### headless run

```
run_container(claude_image, {
    AGENT_TASK: run,
    AGENT_PROMPT: prompt,
}, [session_vol:rw, state_vol:rw])            → exit_code + output
```

## Backend Trait

```rust
trait Backend: Send + Sync {
    // Ref ops
    fn ref_read(&self, side: Side, repo: &str, ref_name: &str)
        -> Result<Option<CommitHash>>;
    fn ref_write(&self, side: Side, repo: &str, ref_name: &str, hash: &CommitHash)
        -> Result<()>;

    // Tree ops
    fn tree_compare(&self, repo_path: &Path, a: &CommitHash, b: &CommitHash)
        -> Result<ContentComparison>;
    fn ancestry_check(&self, repo_path: &Path, a: &CommitHash, b: &CommitHash)
        -> Result<Ancestry>;
    fn merge_trees(&self, repo_path: &Path, ours: &CommitHash, theirs: &CommitHash)
        -> Result<MergeResult>;
    fn checkout(&self, repo_path: &Path, ref_name: &str)
        -> Result<()>;
    fn commit(&self, repo_path: &Path, tree: &TreeHash, parents: &[CommitHash], msg: &str)
        -> Result<CommitHash>;

    // Transport ops
    async fn bundle_create(&self, session: &SessionName, repo: &str)
        -> Result<PathBuf>;
    fn bundle_fetch(&self, repo_path: &Path, bundle: &Path)
        -> Result<CommitHash>;

    // Container ops
    async fn run_container(&self, image: &str, script: &str, mounts: &[Mount])
        -> Result<ContainerOutput>;
    async fn attach_container(&self, image: &str, env: &[(&str, &str)], mounts: &[Mount])
        -> Result<ExitCode>;

    // Control
    fn prompt_user(&self, message: &str) -> Result<bool>;
}
```

Three implementations:
- **RealBackend**: docker + git2 (wraps current SyncEngine methods)
- **MockBackend**: canned responses, records calls (unit tests)
- **DryRunBackend**: returns predicted results (preview)

## Phasing

### Phase 0: Safety net — OPS-2

Defined in `docs/plan/OPS/OPS-2.md`. Derivation tests (23+ pure
cases), merge safety tests (11 git2 cases), parallel assertions on
existing sync_decision_tests. Pins current behavior before anything
moves.

### Phase 1: Fix primitives — OPS-3, OPS-4, OPS-5

Defined in `docs/plan/OPS/OPS-3.md` (inject cleanup),
`docs/plan/OPS/OPS-4.md` (merge guard), `docs/plan/OPS/OPS-5.md`
(clone cleanup). All parallelizable. Each makes one atomic op safe
to fail independently.

**Note:** OPS-3 (inject cleanup) may fix the live inject bug observed
during testing — container HEAD stayed at 280e53a after push. The
inject script's error handling should be verified manually after
this fix lands.

### Phase 2: Define VM types

Types only. No behavior.

**2a.** `Op` enum — 12 variants matching the primitives above.
**2b.** `OpResult` enum — one variant per op outcome.
**2c.** `RepoVM` struct — the per-repo state.
**2d.** `SyncVM` struct — holds repos + backend + trace.
**2e.** `Backend` trait — 12 methods matching the primitives.
**2f.** `Side` enum — `Container | Host`.
**2g.** Pre/postconditions — `Op::check(&RepoVM)` and
`Op::apply(&mut RepoVM, &OpResult)` as pure functions.

### Phase 3: Mock backend + pure VM tests

**3a.** MockBackend implementation.
**3b.** Single-op state transition tests for all 12 primitives.
**3c.** Transactional failure: backend error → VM state unchanged.
**3d.** Multi-op programs: extract sequence (bundle_create →
bundle_fetch → ref_write), merge sequence (ref_read → checkout →
merge_trees → commit → ref_write).

### Phase 4: Compound programs

Define the compound ops as functions that return `Vec<Op>`:

**4a.** `ops_observe(repos)` → scan + ref_reads + comparisons
**4b.** `ops_extract(repo)` → bundle_create + bundle_fetch + ref_write
**4c.** `ops_inject(repo, branch)` → run_container with merge script
**4d.** `ops_merge(repo, from, to, squash)` → ref_read + checkout +
merge_trees + commit + ref_write
**4e.** `ops_clone(repo)` → run_container with clone script
**4f.** `ops_merge_into_volume(repo, branch)` → run_container
**4g.** `ops_launch_reconciliation(repo)` → attach_container
**4h.** `ops_start_session()` → attach_container with work task

Tests: assert each compound returns the right primitive sequence.
Pure, no Docker.

### Phase 5: Program generators

Read VM state, emit compound sequences:

**5a.** `plan_push(vm)` → for each repo needing push: ops_inject
**5b.** `plan_pull(vm)` → ops_extract per repo + ops_observe +
ops_merge per repo. Handle reconcile branching.
**5c.** `plan_sync(vm)` → plan_push + plan_pull
**5d.** Generator tests: construct VM state, assert emitted programs.

### Phase 6: Real backend adapter

**6a.** RealBackend wrapping current SyncEngine methods.
**6b.** ref_read/ref_write via git2.
**6c.** bundle_create/bundle_fetch via throwaway containers + git CLI.
**6d.** run_container/attach_container via bollard.
**6e.** Equivalence test: same scenario through old dispatch and
through VM+RealBackend, assert identical results. Requires Docker.

### Phase 7: Preview + interpreter

**7a.** Op::display() for each of 12 primitives.
**7b.** Compound-level preview: group primitives into logical steps
for display ("extract synapse (3 commits)" rather than
"bundle_create synapse → bundle_fetch synapse → ref_write session").
**7c.** `vm.run(program)`: walk ops, check preconditions, call backend,
apply postconditions, record trace.
**7d.** Trace rendering: show what happened per step.

### Phase 8: Migrate commands

**8a.** cmd_push: plan → generate → preview → confirm → run → render.
**8b.** cmd_pull: same, with re-observe handling.
**8c.** cmd_sync: same, push phase then pull phase.
**8d.** cmd_start: ops_start_session through VM.
**8e.** cmd_extract: extract-only program.
**8f.** Integration tests against Docker.

### Phase 9: Clean up

**9a.** Delete dispatch_pull, dispatch_push, execute_sync, execute_push.
**9b.** Delete SyncDecision, SkipReason, BlockReason, sync_decision().
**9c.** Delete old result types, old renderer.
**9d.** Delete tests that exercise dead code.
**9e.** Final: `cargo test`, zero unused warnings on sync types.

## Ordering

```
Phase 0  (safety net)
  │
Phase 1a ── 1b ── 1c  (parallel cleanup fixes)
  │
Phase 2  (VM types — gates everything)
  │
  ├── Phase 3  (mock backend + pure tests)
  ├── Phase 4  (compound programs)
  │
Phase 5  (program generators — needs 3+4)
Phase 6  (real backend — needs 2 only)
  │
Phase 7  (preview + interpreter — needs 3+4+5)
  │
Phase 8  (migrate commands — needs 5+6+7)
  │
Phase 9  (delete old code)
```

## Risk

Incremental. At every phase boundary, `cargo test` passes and the
binary works. Old and new coexist until Phase 9.

Biggest risk: Phase 8b (cmd_pull). Pull has re-plan loop, per-repo
interactive choices, agent reconciliation. Mitigated by Phase 0b
merge tests + Phase 3d program tests + Phase 5b generator tests.

The primitive-level VM is more work than compound-level (12 ops vs 8)
but buys: inject can eventually be decomposed into bundle ops instead
of run_container (eliminating a throwaway container per push),
container lifecycle is explicit and testable, and the mock backend
tests each git operation individually.
