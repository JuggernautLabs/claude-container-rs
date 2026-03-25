# MIRROR-4: --branch and --all-branches Flags on Pull

blocked_by: [MIRROR-3]
unlocks: [MIRROR-5]

## Problem

Current `pull` only operates on the session branch. Users want to pull additional branches from the container — or all of them — with the same preview → confirm → execute flow.

## Scope

### CLI — additive flags on existing pull

```bash
git-sandbox pull -s hypno                                    # current: session branch only
git-sandbox pull -s hypno main                               # current: extract + merge into main
git-sandbox pull -s hypno --branch develop                   # ALSO mirror develop branch
git-sandbox pull -s hypno --branch main,develop,feature-x    # mirror these branches
git-sandbox pull -s hypno --all-branches                     # mirror every branch in container
git-sandbox pull -s hypno --all-branches --dry-run           # preview what all-branches would do
```

`--branch` and `--all-branches` are additive to the existing pull. The session branch extract still happens. The branch flags add direct branch mirroring on top.

### Behavior per branch

For each branch specified by `--branch` or `--all-branches`:

| Container | Host | Action | Prompt? |
|-----------|------|--------|---------|
| `main` at `abc` | `main` at `abc` | Skip | No |
| `main` at `def` | `main` at `abc`, ff possible | Fast-forward | No |
| `main` at `def` | `main` at `abc`, diverged | Force update | Yes |
| `feature-x` exists | no `feature-x` | Create on host | No |
| no `feature-x` | `feature-x` exists | Warn, skip | No (unless `--all-branches`) |
| `main` rebased | old SHA | Force update | Yes |

With `--all-branches`: deletions and force updates happen without individual prompts (one confirmation for the whole operation).

### Preview (always shown, even without --dry-run)

```
git-sandbox pull -s hypno --all-branches
──── pull: hypno → host ────
  ✓ 3 ready, 1 pending merge
  ...current session branch output...

  branches:
    hypermemetic/synapse:
      ✓ main         abc123 → def456 (3 commits, ff)
      · develop      same
      + feature-x    new (aaa111, 5 commits)
      ⚠ hotfix       force update (container rebased)
    hypermemetic/plexus-core:
      ✓ main         same
      - old-feature   deleted in container

  Branch sync: 2 update, 1 create, 1 force, 1 delete
  Execute? [Y/n]
```

### Extraction mechanism

Same bundle extraction as current, but for each branch:

```bash
# In throwaway container, bundle specific branch:
git bundle create /bundles/repo-branch.bundle branch_name

# On host, fetch and update:
git fetch /path/to/bundle branch_name
git branch -f branch_name FETCH_HEAD
```

For `--all-branches`, bundle everything: `git bundle create /bundles/repo.bundle --all`

### No mode switching

`pull` does NOT change behavior based on session config. The flags are explicit every time:
- No `--branch` → current extract behavior
- `--branch X` → extract + mirror branch X
- `--all-branches` → extract + mirror all

`track-branches` (MIRROR-2) becomes optional sugar — it pre-sets the default `--branch` list so you don't have to type it every time.

## Files to modify

- `src/sync/mod.rs` — `mirror_branches()` function, per-branch extraction
- `src/main.rs` — `cmd_pull` checks for tracked branches, routes to mirror
- `src/render.rs` — mirror preview rendering
- `src/types/action.rs` — `MirrorAction`, `BranchMirrorResult`
