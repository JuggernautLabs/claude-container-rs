# MIRROR-6: Push Branches into Container

blocked_by: [MIRROR-3]
unlocks: []

## Problem

Currently `push` injects one branch (the target) into the container via `git fetch + merge`. With branch mirroring, you want to push multiple host branches into the container so Claude can see them — feature branches, develop, PRs, etc.

## Scope

### CLI

```bash
# Push specific branches into container
git-sandbox push -s hypno --branches main,develop,feature-x

# Push all tracked branches (reverse of pull)
git-sandbox push -s hypno

# Push a branch that doesn't exist in container yet
git-sandbox push -s hypno --branches new-feature
```

### Behavior

For each branch to push:

| Host | Container | Action |
|------|-----------|--------|
| `main` at `abc` | `main` at `abc` | Skip |
| `main` at `def` | `main` at `abc` | Inject: fetch + ff or merge |
| `feature-x` at `aaa` | no `feature-x` | Create branch in container |
| no `feature-x` | `feature-x` at `bbb` | Skip (don't delete container work) |

### Mechanism

Reuse `inject()` for existing branches. For new branches:

```bash
# Throwaway container:
cd /session/$repo
git remote add _upstream /upstream
git fetch _upstream $branch
git branch $branch _upstream/$branch
git remote remove _upstream
```

### Integration with tracked branches

If the session has tracked branches, `push` without `--branches` pushes all tracked branches. This is the reverse of `pull` — host → container for tracked branches.

### Preview

```
git-sandbox push -s hypno --dry-run
──── push: host → hypno ────
  hypermemetic/synapse:
    → main       host:def456 → container (2 commits)
    + develop    create in container (host:aaa111)
    · feature-x  same
```

## Files to modify

- `src/sync/mod.rs` — `push_branches()`, create-branch-in-volume
- `src/main.rs` — `--branches` flag on push, multi-branch routing
- `src/container/mod.rs` — may need multi-branch inject
