# MIRROR-4: Mirror Pull — Copy Container Branches to Host

blocked_by: [MIRROR-2, MIRROR-3]
unlocks: []

## Problem

Current `pull` extracts to a session branch then optionally squash-merges into a target. Branch mirroring is different: for each tracked branch, make the host branch point to the same commit as the container branch. No merge — a direct ref update.

## Scope

### Behavior

For each tracked branch in each project repo:

| Container | Host | Action |
|-----------|------|--------|
| `main` at `abc123` | `main` at `abc123` | Skip (same) |
| `main` at `def456` | `main` at `abc123` | Update host → `def456` (ff or force) |
| `feature-x` at `aaa` | no `feature-x` | Create `feature-x` on host |
| no `feature-x` | `feature-x` at `bbb` | Warn (or delete with `--all`) |
| `main` rebased | `main` at old SHA | Warn (or force update with `--all`) |

### Extraction mechanism

Same as current: git bundle from container → git fetch on host. But instead of creating a session branch, we update the ACTUAL branch:

```rust
// Current: creates refs/heads/{session_name}
repo.reference(&session_ref, new_head_oid, true, "cc: extract")?;

// Mirror: updates refs/heads/{branch_name} directly
repo.reference(&format!("refs/heads/{}", branch_name), new_head_oid, true, "gs: mirror")?;
```

### Safety

**Non-destructive by default:**
- Fast-forward updates: apply silently
- Force updates (rebase/rewrite): prompt `Branch 'main' was rewritten in container. Update host? [Y/n]`
- Deletions: prompt `Branch 'feature-x' deleted in container. Delete on host? [Y/n]`

**With `--all`:**
- All updates applied without prompting
- Deletions applied without prompting
- History rewrites applied without prompting

### Preview

```
git-sandbox pull -s hypno --dry-run
──── mirror: hypno → host ────
  hypermemetic/synapse:
    ✓ main         abc123 → def456 (3 commits, ff)
    ✓ develop      same
    + feature-x    new (aaa111, 5 commits)
    ✗ old-branch   deleted in container

  hypermemetic/plexus-core:
    ✓ main         same
    ⚠ develop      rebased (force update required)
```

### CLI integration

`pull` detects tracked branches mode:
- If `tracked_branches` is set → mirror mode
- If not set → current extract+merge mode (backward compat)

No new command needed — `pull` adapts based on session config.

## Files to modify

- `src/sync/mod.rs` — `mirror_branches()` function, per-branch extraction
- `src/main.rs` — `cmd_pull` checks for tracked branches, routes to mirror
- `src/render.rs` — mirror preview rendering
- `src/types/action.rs` — `MirrorAction`, `BranchMirrorResult`
