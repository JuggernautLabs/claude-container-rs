# GitOps Semantics

## The two copies

```
HOST                              CONTAINER
├── repo/.git/                    ├── repo/.git/
│   ├── refs/heads/main           │   └── HEAD
│   ├── refs/heads/{session}      │
│   └── refs/claude-container/    │
│       └── squash-base/{session} │
└── working tree                  └── working tree
```

## Operations

### `extract` — container commits → host session branch
Only updates refs/heads/{session}. Does NOT touch main. No docker needed after bundle.

### `merge` — host session branch → host target branch  
Squash-merge or ff. Updates squash-base ref. Host-only git.

### `inject` — host branch → container
FF or merge inside container via docker run. Modifies container volume.

### `pull` = extract + merge
### `push` = inject
### `sync` = classify each repo → extract|inject|reconcile per repo

## Squash tracking
refs/claude-container/squash-base/{session} = last squashed commit.
Next merge only includes commits after the squash-base.
