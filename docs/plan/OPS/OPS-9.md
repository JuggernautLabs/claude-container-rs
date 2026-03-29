---
status: SUPERSEDED by VM epic (docs/plan/VM/VM-1.md)
---

# OPS-9: Preview Renderer — Display Program Before Execution

blocked_by: [OPS-6]
unlocks: [OPS-10]

## Design

Given a `SyncProgram`, render each op as a human-readable line.
Preview reads from the program's ops list plus the precomputed
diffs/trial_merges from the plan.

### Output format

```
────────── push: main → http-gateway (container) ──────────
✓ 1 ready

  ✓ ../synapse — inject 3 commit(s) from main
    container:280e53a  session:280e53a  main:e9c84eb
      13 files changed, +573 -208

────────────────────────────────────────────────────────────
```

```
──────────── pull: http-gateway → main ────────────────────
✓ 2 ready, 1 pending merge

  1. extract ../synapse                    (5 commits)
  2. extract ../plexus                     (2 commits)
  3. re-observe
  4. merge ../synapse → main               (squash, trial: clean)
  5. merge ../plexus → main                (squash, trial: CONFLICT src/lib.rs)

────────────────────────────────────────────────────────────
```

```
──────────── sync: http-gateway ↔ main ────────────────────
  push phase:
    1. inject ../synapse                   (3 commits from main)
  pull phase:
    2. extract ../synapse                  (5 commits)
    3. re-observe
    4. merge ../synapse → main             (squash, trial: clean)

────────────────────────────────────────────────────────────
```

### Implementation

```rust
impl SyncProgram {
    pub fn render_preview(&self) {
        // Header
        render::rule(Some(&self.preview.label));

        // Summary counts
        let summary = self.summarize();
        // ... render summary line ...

        // Numbered op list
        for (i, op) in self.ops.iter().enumerate() {
            let detail = self.preview.op_detail(i);  // diff, trial result
            render_op(i + 1, op, detail);
        }

        // Diffstat section
        // ... reuse existing diffstat rendering ...

        render::rule(None);
    }
}

fn render_op(n: usize, op: &Op, detail: Option<&OpDetail>) {
    match op {
        Op::Extract { repo } => {
            let commits = detail.map(|d| d.commits).unwrap_or(0);
            println!("  {}. extract {}  ({} commits)", n, repo, commits);
        }
        Op::Inject { repo, branch } => {
            let commits = detail.map(|d| d.commits).unwrap_or(0);
            println!("  {}. inject {} → container  ({} commits from {})",
                     n, repo, commits, branch);
        }
        Op::Merge { repo, to_branch, squash, .. } => {
            let trial = detail.and_then(|d| d.trial_result.as_ref());
            let trial_str = match trial {
                Some(files) if files.is_empty() => "trial: clean".green().to_string(),
                Some(files) => format!("trial: CONFLICT {}", files.join(", ")).red().to_string(),
                None => "trial: unknown".dimmed().to_string(),
            };
            let mode = if *squash { "squash" } else { "merge" };
            println!("  {}. {} {} → {}  ({}, {})", n, mode, repo, to_branch, mode, trial_str);
        }
        Op::ReObserve => {
            println!("  {}. {}", n, "re-observe".dimmed());
        }
        Op::LaunchReconciliation { repo, .. } => {
            println!("  {}. {} — launch Claude to resolve conflicts",
                     n, repo.yellow());
        }
        Op::Confirm { message } => {
            println!("  {}. {}", n, message.dimmed());
        }
        _ => {}
    }
}
```

### ProgramPreview

```rust
pub struct ProgramPreview {
    pub label: String,          // "push: main → session"
    pub op_details: Vec<Option<OpDetail>>,  // per-op precomputed data
}

pub struct OpDetail {
    pub commits: u32,
    pub diff: Option<DiffSummary>,
    pub trial_result: Option<Vec<String>>,
    pub hashes: Option<(CommitHash, CommitHash, CommitHash)>,  // container, session, target
}
```

Built during program generation (OPS-8), not during rendering. The
renderer just reads data.

## Replaces

`render::sync_plan_inner()` and `render::sync_plan_directed()`. These
currently read RepoSyncAction fields directly. The new renderer reads
from Op + OpDetail, which is populated by the program generator.

Existing render code can be deleted in OPS-11.

## Test

```rust
#[test]
fn preview_push_shows_inject_ops() {
    let program = SyncProgram::for_push(&plan);
    let output = capture_stdout(|| program.render_preview());
    assert!(output.contains("inject"));
    assert!(!output.contains("merge"));
    assert!(!output.contains("extract"));
}
```

## Acceptance criteria

- Each Op variant has a render line
- Ops are numbered sequentially
- Diffs and trial merge results shown inline
- Push preview shows only inject ops
- Pull preview shows extract → re-observe → merge sequence
- Sync preview shows push phase then pull phase
