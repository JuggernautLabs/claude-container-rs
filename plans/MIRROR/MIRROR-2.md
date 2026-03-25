# MIRROR-2: Track Branches Command

blocked_by: []
unlocks: [MIRROR-4, MIRROR-5]

## Problem

Currently each session has a single implicit branch relationship: container HEAD → session branch → target branch. There's no way to tell a session "also sync these other branches."

## Scope

Add `session track-branches` command that stores which branches to mirror.

### CLI

```bash
# Track specific branches
git-sandbox session -s hypno track-branches main develop feature-x

# Track all branches in container
git-sandbox session -s hypno track-branches --all

# Show tracked branches
git-sandbox session -s hypno track-branches

# Stop tracking a branch
git-sandbox session -s hypno track-branches --remove feature-x

# Stop tracking all (back to default: session branch only)
git-sandbox session -s hypno track-branches --clear
```

### Storage

In `.claude-projects.yml` (or a new `.git-sandbox-mirrors.yml`):

```yaml
tracked_branches:
  mode: explicit  # or "all"
  branches:
    - main
    - develop
    - feature-x
```

Or per-repo:

```yaml
projects:
  hypermemetic/synapse:
    path: /path/to/synapse
    role: project
    tracked_branches:
      - main
      - develop
```

### Display

`session show` includes tracked branches:

```
session: hypno
  tracked branches: main, develop, feature-x
  # or
  tracked branches: all
```

## Files to modify

- `src/types/config.rs` — add `tracked_branches` field to SessionConfig or ProjectConfig
- `src/main.rs` — add `TrackBranches` to SessionAction enum
- `src/session/mod.rs` — read/write tracked branches config
- `src/render.rs` — show tracked branches in session info
