# MIRROR-5: --all Mode — Container as Full Authority

blocked_by: [MIRROR-4]
unlocks: []

## Problem

Without `--all`, mirror pull prompts for every destructive operation (force updates, deletions). With `--all`, the container is the complete source of truth — the host should match exactly, no questions asked.

## Scope

### Behavior with `--all`

```bash
git-sandbox pull -s hypno --all
```

For each repo:
1. Get container branch list (from MIRROR-3 snapshot)
2. Get host branch list
3. For branches in container: create or update on host (force if needed)
4. For branches on host NOT in container: **delete on host**
5. No prompts

This is essentially `git push --mirror` in reverse.

### Safety

`--all` is destructive. It should:
- Require `--yes` or interactive confirmation for the first use per session
- Show a preview of what will be deleted/force-updated before executing
- Store a marker that `--all` was used (so subsequent pulls know the mode)

### Preview

```
git-sandbox pull -s hypno --all --dry-run
──── full mirror: hypno → host ────
  hypermemetic/synapse:
    ✓ main         → def456 (ff)
    ✓ develop      → aaa111 (ff)
    + feature-x    create (bbb222)
    - old-branch   DELETE
    ⚠ hotfix       force update (rebased)

  3 updates, 1 create, 1 delete, 1 force update
  Execute? [Y/n]
```

### Protection

Never delete `main` or `master` unless the container also doesn't have them (edge case). Protected branch list:

```rust
const PROTECTED_BRANCHES: &[&str] = &["main", "master"];
```

Even with `--all`, deleting a protected branch requires explicit `--force`.

## Files to modify

- `src/sync/mod.rs` — extend `mirror_branches()` with delete logic
- `src/main.rs` — `--all` flag on pull, route to full mirror
- `src/types/config.rs` — track-branches `mode: all` storage
