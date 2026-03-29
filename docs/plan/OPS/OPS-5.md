# OPS-5: Cleanup — Partial Clone Leaves Stale Directory

blocked_by: []
unlocks: [OPS-6]

## Bug

`clone_into_volume()` runs `git clone` inside a throwaway container.
If the clone fails partway (network error, disk full, permission issue),
the container exits non-zero and is removed, but the partial repo
directory remains in the volume at `/session/<repo_name>`.

Next `clone_into_volume()` fails with "already exists." The user has
to manually delete the directory from the volume (which requires
another container).

## Fix

Add cleanup to the clone script: on failure, remove the target
directory before exiting.

Current script (lines ~1090-1110):
```bash
git clone --no-local /upstream /workspace/{name} && chown -R {uid}:{gid} /workspace/{name}
```

Change to:
```bash
git clone --no-local /upstream /workspace/{name}
clone_rc=$?
if [ $clone_rc -ne 0 ]; then
    rm -rf /workspace/{name}
    exit $clone_rc
fi
chown -R {uid}:{gid} /workspace/{name}
```

Also: before cloning, check if directory already exists and remove it
(handles recovery from a previous failed clone):
```bash
[ -d /workspace/{name} ] && rm -rf /workspace/{name}
git clone --no-local /upstream /workspace/{name}
...
```

## Test

```rust
#[tokio::test]
#[ignore]
async fn clone_failure_cleans_up_directory() {
    // Setup: session volume, host repo path that doesn't exist (clone will fail)
    // clone_into_volume() should fail
    // Then: list volume contents — no partial directory
    // Then: clone_into_volume() with valid path — should succeed
}
```

## Acceptance criteria

- Failed clone removes partial directory from volume
- Retry after failed clone succeeds
- Pre-existing stale directory is cleaned before clone attempt
