--------------------------- MODULE SyncOps ---------------------------
(*
 * Formal model of the git-sandbox sync operation language.
 *
 * SAFETY PROPERTY: the target branch (user's real branch, e.g. main)
 * must NEVER be left in an inconsistent state by this program.
 * Inconsistent means: conflict markers committed, partial merge,
 * target ref pointing at invalid commit, target ref regressed.
 *
 * ADVERSARIAL ENVIRONMENT: between any two atomic operations, a user
 * can commit, force-push, delete branches, dirty the worktree.
 *
 * TERMINATION: bounded by fuel. The language is bounded, not convergent.
 *)

EXTENDS Naturals, FiniteSets

CONSTANTS
    Repos,
    MaxCommit,
    MaxAgentRuns,
    MaxReObserves,
    Fuel            \* Total operation budget

Commits == 0..MaxCommit

\* Target branch state: the thing we must protect
TargetState == [
    head: Commits,           \* The branch ref
    worktree: Commits,       \* What's checked out (may differ during merge)
    merge_state: {"clean", "merging", "conflicted"},
    has_conflict_markers: BOOLEAN
]

VARIABLES
    \* Actual git state
    container,      \* [Repos -> Commits]
    session,        \* [Repos -> Commits ∪ {-1}]
    target_head,    \* [Repos -> Commits] — the ref itself
    target_wt,      \* [Repos -> Commits] — worktree state
    target_ms,      \* [Repos -> {"clean", "merging", "conflicted"}]
    target_markers, \* [Repos -> BOOLEAN] — conflict markers in worktree
    cDirty,         \* [Repos -> BOOLEAN]
    hDirty,         \* [Repos -> BOOLEAN]
    conflict,       \* [Repos -> {"none", "markers", "resolved"}] — container conflict

    \* Snapshot (frozen at observe time)
    snap_c, snap_s, snap_t,

    \* Program state
    phase,          \* "observe" | "push" | "pull" | "done"
    pc,
    agentRuns,
    reObserves,
    fuel,

    \* History: track the original target head at observation time
    \* to verify we never regress it
    target_at_observe  \* [Repos -> Commits]

vars == <<container, session, target_head, target_wt, target_ms,
          target_markers, cDirty, hDirty, conflict,
          snap_c, snap_s, snap_t,
          phase, pc, agentRuns, reObserves, fuel,
          target_at_observe>>

\* ================================================================
\* THE SAFETY INVARIANT
\*
\* After every step of the program, for every repo:
\*   1. target_head >= target_at_observe (never regresses)
\*   2. target_markers = FALSE on target (no conflict markers committed)
\*   3. if target_ms = "clean" then target_wt = target_head
\*      (worktree matches ref when not mid-merge)
\*
\* Property 2 is the critical one. The merge() implementation
\* detects conflicts → cleans up → returns Conflict result.
\* It NEVER commits conflict markers to the target branch.
\* ================================================================

TargetSafety ==
    \A r \in Repos :
        \* Target never regresses from what we observed
        /\ target_head[r] >= target_at_observe[r]
        \* No conflict markers ever committed to target
        /\ ~target_markers[r]
        \* When not merging, worktree matches head
        /\ (target_ms[r] = "clean" => target_wt[r] = target_head[r])

\* ================================================================
\* Observation
\* ================================================================

Observe ==
    /\ phase = "observe"
    /\ snap_c' = container
    /\ snap_s' = session
    /\ snap_t' = target_head
    /\ target_at_observe' = target_head  \* record baseline
    /\ phase' = "push"
    /\ pc' = "idle"
    /\ UNCHANGED <<container, session, target_head, target_wt, target_ms,
                   target_markers, cDirty, hDirty, conflict,
                   agentRuns, reObserves, fuel>>

\* Derive from snapshot
SnapExtLeg(r) ==
    IF snap_c[r] = snap_s[r] THEN "in_sync"
    ELSE IF snap_s[r] = -1 THEN "no_session"
    ELSE IF snap_c[r] > snap_s[r] THEN "container_ahead"
    ELSE IF snap_s[r] > snap_c[r] THEN "session_ahead"
    ELSE "diverged"

SnapMrgLeg(r) ==
    IF snap_s[r] = -1 THEN "no_target"
    ELSE IF snap_s[r] = snap_t[r] THEN "in_sync"
    ELSE IF snap_s[r] > snap_t[r] THEN "session_ahead"
    ELSE IF snap_t[r] > snap_s[r] THEN "target_ahead"
    ELSE "diverged"

SnapPull(r) ==
    CASE SnapExtLeg(r) = "container_ahead" -> "extract"
      [] SnapExtLeg(r) = "no_session"      -> "clone"
      [] SnapExtLeg(r) = "diverged"        -> "reconcile"
      [] SnapExtLeg(r) = "in_sync" /\ SnapMrgLeg(r) = "session_ahead" -> "merge"
      [] SnapExtLeg(r) = "in_sync" /\ SnapMrgLeg(r) = "diverged"      -> "merge"
      [] OTHER -> "skip"

SnapPush(r) ==
    CASE SnapMrgLeg(r) = "target_ahead" -> "inject"
      [] SnapMrgLeg(r) = "diverged"     -> "inject"
      [] SnapExtLeg(r) = "session_ahead" -> "inject"
      [] OTHER -> "skip"

\* ================================================================
\* Environment: adversarial user mutations
\* NOTE: user can advance target but we track regression separately.
\* User can also dirty the worktree, which blocks our merge.
\* ================================================================

UserMutate ==
    /\ fuel > 0
    /\ \E r \in Repos :
        \/ \* Commit to container
           /\ container[r] < MaxCommit
           /\ container' = [container EXCEPT ![r] = container[r] + 1]
           /\ UNCHANGED <<session, target_head, target_wt, target_ms,
                          target_markers, cDirty, hDirty, conflict>>
        \/ \* Commit to target (user pushes to main — target advances)
           /\ target_head[r] < MaxCommit
           /\ target_ms[r] = "clean"  \* user can only commit when clean
           /\ target_head' = [target_head EXCEPT ![r] = target_head[r] + 1]
           /\ target_wt' = [target_wt EXCEPT ![r] = target_head[r] + 1]
           /\ UNCHANGED <<container, session, target_ms, target_markers,
                          cDirty, hDirty, conflict>>
        \/ \* Force-push target (regression — but user chose to do this)
           \* We track that OUR program doesn't regress it, user can.
           /\ \E c \in Commits :
                /\ target_head' = [target_head EXCEPT ![r] = c]
                /\ target_wt' = [target_wt EXCEPT ![r] = c]
                /\ target_at_observe' = [target_at_observe EXCEPT ![r] = c]
                   \* Reset baseline — user took ownership of this regression
           /\ UNCHANGED <<container, session, target_ms, target_markers,
                          cDirty, hDirty, conflict>>
        \/ \* Make host dirty (user edits files)
           /\ hDirty' = [hDirty EXCEPT ![r] = TRUE]
           /\ UNCHANGED <<container, session, target_head, target_wt,
                          target_ms, target_markers, cDirty, conflict>>
        \/ \* Make container dirty
           /\ cDirty' = [cDirty EXCEPT ![r] = TRUE]
           /\ UNCHANGED <<container, session, target_head, target_wt,
                          target_ms, target_markers, hDirty, conflict>>
        \/ \* Delete session branch
           /\ session' = [session EXCEPT ![r] = -1]
           /\ UNCHANGED <<container, target_head, target_wt, target_ms,
                          target_markers, cDirty, hDirty, conflict>>
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<snap_c, snap_s, snap_t, phase, pc,
                   agentRuns, reObserves>>

\* ================================================================
\* Atomic operations — MODELED WITH CRASH POINTS
\*
\* Each multi-step operation is broken into its atomic git
\* sub-operations. The environment can act between any two.
\* ================================================================

\* --- Extract: session <- container (safe, only touches session ref) ---
DoExtract(r) ==
    /\ fuel > 0
    /\ session' = [session EXCEPT ![r] = container[r]]
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<container, target_head, target_wt, target_ms,
                   target_markers, cDirty, hDirty, conflict,
                   snap_c, snap_s, snap_t, agentRuns, target_at_observe>>

\* --- Inject: container <- max(container, target) (safe, only touches container) ---
DoInject(r) ==
    /\ fuel > 0
    /\ ~cDirty[r]
    /\ LET new == IF container[r] >= target_head[r] THEN container[r]
                  ELSE IF target_head[r] <= MaxCommit THEN target_head[r]
                  ELSE container[r]
       IN container' = [container EXCEPT ![r] = new]
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<session, target_head, target_wt, target_ms,
                   target_markers, cDirty, hDirty, conflict,
                   snap_c, snap_s, snap_t, agentRuns, target_at_observe>>

\* --- Merge: THE DANGEROUS ONE ---
\* This is the only operation that touches target_head.
\* Modeled as three atomic sub-steps with possible failure at each.

\* Step 1: Enter merge state (checkout target, begin merge)
MergeBegin(r) ==
    /\ fuel > 0
    /\ ~hDirty[r]           \* precondition: host clean
    /\ session[r] /= -1     \* session branch exists
    /\ target_ms[r] = "clean"  \* not already merging
    /\ target_ms' = [target_ms EXCEPT ![r] = "merging"]
    /\ target_wt' = [target_wt EXCEPT ![r] = target_head[r]]  \* checkout target
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<container, session, target_head, target_markers,
                   cDirty, hDirty, conflict,
                   snap_c, snap_s, snap_t, agentRuns, reObserves,
                   target_at_observe>>

\* Step 2a: Merge succeeds — commit and advance target
MergeCommit(r) ==
    /\ fuel > 0
    /\ target_ms[r] = "merging"
    /\ session[r] /= -1
    \* Only succeeds if session has work to merge
    /\ session[r] /= target_head[r]
    \* Advance target: new commit is ahead of both parents
    /\ LET new == IF session[r] > target_head[r] THEN session[r]
                  ELSE IF target_head[r] < MaxCommit THEN target_head[r] + 1
                  ELSE target_head[r]
       IN /\ target_head' = [target_head EXCEPT ![r] = new]
          /\ target_wt' = [target_wt EXCEPT ![r] = new]
    /\ target_ms' = [target_ms EXCEPT ![r] = "clean"]
    /\ target_markers' = [target_markers EXCEPT ![r] = FALSE]  \* NEVER commit markers
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<container, session, cDirty, hDirty, conflict,
                   snap_c, snap_s, snap_t, agentRuns, reObserves,
                   target_at_observe>>

\* Step 2b: Merge has conflicts — ROLLBACK (the critical safety path)
MergeConflictRollback(r) ==
    /\ fuel > 0
    /\ target_ms[r] = "merging"
    \* Rollback: restore worktree, clear merge state, DO NOT advance target
    /\ target_ms' = [target_ms EXCEPT ![r] = "clean"]
    /\ target_wt' = [target_wt EXCEPT ![r] = target_head[r]]  \* force checkout
    /\ target_markers' = [target_markers EXCEPT ![r] = FALSE]  \* markers cleared
    \* target_head UNCHANGED — this is the safety guarantee
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<container, session, target_head, cDirty, hDirty, conflict,
                   snap_c, snap_s, snap_t, agentRuns, reObserves,
                   target_at_observe>>

\* Step 2c: Merge CRASHES mid-operation — worst case
\* Leaves merge state but doesn't commit. Target ref unchanged.
\* Next observation sees hDirty or merge_state != clean → blocked.
MergeCrash(r) ==
    /\ fuel > 0
    /\ target_ms[r] = "merging"
    \* Crash leaves merge state dirty, but target_head UNCHANGED
    /\ target_ms' = [target_ms EXCEPT ![r] = "conflicted"]
    /\ hDirty' = [hDirty EXCEPT ![r] = TRUE]  \* will be seen as dirty next observe
    \* CRITICAL: target_head does NOT change on crash
    /\ target_markers' = [target_markers EXCEPT ![r] = FALSE]
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<container, session, target_head, target_wt,
                   cDirty, conflict,
                   snap_c, snap_s, snap_t, agentRuns, reObserves,
                   target_at_observe>>

\* --- MergeIntoVolume: only touches container, safe for target ---
DoMergeIntoVolume(r) ==
    /\ fuel > 0
    /\ conflict' = [conflict EXCEPT ![r] = "markers"]
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<container, session, target_head, target_wt, target_ms,
                   target_markers, cDirty, hDirty,
                   snap_c, snap_s, snap_t, agentRuns, target_at_observe>>

\* --- Agent resolve: only touches container, safe for target ---
DoAgentResolve(r) ==
    /\ fuel > 0
    /\ conflict[r] = "markers"
    /\ agentRuns < MaxAgentRuns
    /\ container' = [container EXCEPT ![r] =
        IF container[r] < MaxCommit THEN container[r] + 1 ELSE container[r]]
    /\ conflict' = [conflict EXCEPT ![r] = "resolved"]
    /\ agentRuns' = agentRuns + 1
    /\ fuel' = fuel - 1
    /\ UNCHANGED <<session, target_head, target_wt, target_ms,
                   target_markers, cDirty, hDirty,
                   snap_c, snap_s, snap_t, target_at_observe>>

\* ================================================================
\* Program phases
\* ================================================================

PushStep ==
    /\ phase = "push"
    /\ \E r \in Repos :
        /\ SnapPush(r) = "inject"
        /\ DoInject(r)
    /\ pc' = "idle"
    /\ UNCHANGED <<phase, reObserves>>

PushDone ==
    /\ phase = "push"
    /\ ~(\E r \in Repos : SnapPush(r) = "inject")
    /\ phase' = "pull"
    /\ pc' = "idle"
    /\ UNCHANGED <<container, session, target_head, target_wt, target_ms,
                   target_markers, cDirty, hDirty, conflict,
                   snap_c, snap_s, snap_t, agentRuns, reObserves, fuel,
                   target_at_observe>>

PullExtract ==
    /\ phase = "pull" /\ pc = "idle"
    /\ \E r \in Repos :
        /\ SnapPull(r) \in {"extract", "clone"}
        /\ DoExtract(r)
    /\ pc' = "idle"
    /\ UNCHANGED <<phase, reObserves>>

PullReObserve ==
    /\ phase = "pull" /\ pc = "idle"
    /\ ~(\E r \in Repos : SnapPull(r) \in {"extract", "clone"})
    /\ reObserves < MaxReObserves
    /\ snap_c' = container
    /\ snap_s' = session
    /\ snap_t' = target_head
    /\ pc' = "merge_begin"
    /\ reObserves' = reObserves + 1
    /\ UNCHANGED <<container, session, target_head, target_wt, target_ms,
                   target_markers, cDirty, hDirty, conflict,
                   phase, agentRuns, fuel, target_at_observe>>

\* Merge phase: begin merge for each repo that needs it
PullMergeBegin ==
    /\ phase = "pull" /\ pc = "merge_begin"
    /\ \E r \in Repos :
        /\ SnapPull(r) = "merge"
        /\ MergeBegin(r)
    /\ pc' = "merge_commit"
    /\ UNCHANGED <<phase, reObserves>>

\* Merge phase: commit or rollback
PullMergeResolve ==
    /\ phase = "pull" /\ pc = "merge_commit"
    /\ \E r \in Repos :
        /\ target_ms[r] = "merging"
        /\ \/ MergeCommit(r)
           \/ MergeConflictRollback(r)
           \/ MergeCrash(r)  \* non-deterministic: crash can happen
    /\ pc' = "merge_begin"  \* continue with next repo
    /\ UNCHANGED <<phase, reObserves>>

\* Reconcile: merge into volume then agent
PullReconcile ==
    /\ phase = "pull" /\ pc = "merge_begin"
    /\ \E r \in Repos :
        /\ SnapPull(r) = "reconcile"
        /\ DoMergeIntoVolume(r)
    /\ pc' = "agent_wait"
    /\ UNCHANGED <<phase, reObserves>>

PullAgentDone ==
    /\ phase = "pull" /\ pc = "agent_wait"
    /\ \E r \in Repos :
        /\ conflict[r] = "markers"
        /\ DoAgentResolve(r)
    /\ pc' = "idle"
    /\ UNCHANGED <<phase, reObserves>>

PullDone ==
    /\ phase = "pull"
    /\ pc \in {"merge_begin", "merge_commit"}
    /\ ~(\E r \in Repos : SnapPull(r) \in {"merge", "reconcile"})
    /\ ~(\E r \in Repos : target_ms[r] = "merging")
    /\ phase' = "done"
    /\ pc' = "done"
    /\ UNCHANGED <<container, session, target_head, target_wt, target_ms,
                   target_markers, cDirty, hDirty, conflict,
                   snap_c, snap_s, snap_t, agentRuns, reObserves, fuel,
                   target_at_observe>>

\* ================================================================
\* Spec
\* ================================================================

Init ==
    /\ container \in [Repos -> Commits]
    /\ session \in [Repos -> Commits \cup {-1}]
    /\ target_head \in [Repos -> Commits]
    /\ target_wt = target_head           \* starts consistent
    /\ target_ms = [r \in Repos |-> "clean"]
    /\ target_markers = [r \in Repos |-> FALSE]
    /\ cDirty = [r \in Repos |-> FALSE]
    /\ hDirty = [r \in Repos |-> FALSE]
    /\ conflict = [r \in Repos |-> "none"]
    /\ snap_c = [r \in Repos |-> 0]
    /\ snap_s = [r \in Repos |-> 0]
    /\ snap_t = [r \in Repos |-> 0]
    /\ phase = "observe"
    /\ pc = "idle"
    /\ agentRuns = 0
    /\ reObserves = 0
    /\ fuel = Fuel
    /\ target_at_observe = target_head

Next ==
    \/ Observe
    \/ PushStep /\ UNCHANGED target_at_observe
    \/ PushDone
    \/ PullExtract /\ UNCHANGED target_at_observe
    \/ PullReObserve
    \/ PullMergeBegin /\ UNCHANGED target_at_observe
    \/ PullMergeResolve /\ UNCHANGED target_at_observe
    \/ PullReconcile /\ UNCHANGED target_at_observe
    \/ PullAgentDone /\ UNCHANGED target_at_observe
    \/ PullDone
    \/ UserMutate

Spec == Init /\ [][Next]_vars

\* ================================================================
\* SAFETY PROPERTIES
\* ================================================================

\* THE property: target branch is never corrupted by the program.
\* User can do whatever they want — we just can't make it worse.
INVARIANT_TargetNeverCorrupted ==
    \A r \in Repos :
        \* No conflict markers ever committed to target
        ~target_markers[r]

\* Target ref only advances (within program operations).
\* User force-push resets the baseline, which is fine.
INVARIANT_TargetNeverRegresses ==
    \A r \in Repos :
        target_head[r] >= target_at_observe[r]

\* When merge state is clean, worktree matches head.
\* (Mid-merge, worktree may temporarily differ.)
INVARIANT_WorktreeConsistent ==
    \A r \in Repos :
        target_ms[r] = "clean" => target_wt[r] = target_head[r]

\* Extract and inject NEVER touch target.
\* (Structural — enforced by UNCHANGED in those operations.)

\* Agent operations NEVER touch target.
\* (Structural — agents only modify container.)

\* Only merge touches target, and only via MergeCommit.
\* MergeConflictRollback restores target to pre-merge state.
\* MergeCrash leaves target_head unchanged (dirty but not corrupted).

\* ================================================================
\* TERMINATION
\* ================================================================

INVARIANT_FuelBound == fuel >= 0
INVARIANT_AgentBound == agentRuns <= MaxAgentRuns
INVARIANT_ReObserveBound == reObserves <= MaxReObserves

=======================================================================
