# GS-9: Shell Safety — Command Escaping & Exec

blocked_by: []
unlocks: []

## Problem
1. `session exec` joins command args with spaces (`command.join(" ")`) — breaks on quotes, pipes, redirects
2. Scripts passed to throwaway containers via `format!()` don't escape user input
3. `write_config()` escapes single quotes in YAML via `replace('\'', "'\\''")` — not robust
4. `base64_encode()` shells out to `base64` command with fallback to raw string — breaks on special chars

## Scope
- Fix exec to pass args correctly (not joined)
- Use base64 crate instead of shelling out
- Validate/escape all strings passed into container scripts
- write_config: use docker cp or env var instead of shell echo

## TDD Plan

### Tests to write FIRST (in tests/shell_safety_test.rs):

```rust
#[test]
fn exec_handles_quoted_args() {
    // Command: ["echo", "hello world"]
    // Assert: executes correctly, not split on space
}

#[test]
fn exec_handles_special_chars() {
    // Command: ["echo", "it's a \"test\" $HOME"]
    // Assert: literal output, no shell expansion
}

#[test]
fn base64_encode_roundtrips() {
    // Encode + decode preserves: quotes, newlines, unicode, empty string
}

#[test]
fn write_config_handles_special_yaml() {
    // Config with repo names containing quotes, colons, special chars
    // Write + read roundtrips correctly
}
```

## Files to modify
- `src/main.rs` — `cmd_session_exec()`: don't join, use exec array
- `src/container/mod.rs` — replace `base64_encode` shell-out with base64 crate
- `src/session/mod.rs` — `write_config()`: use docker cp or proper escaping
- `Cargo.toml` — add `base64` crate

## Acceptance criteria
- `session exec echo "hello world"` works correctly
- `session exec ls -la /path/with spaces/` works correctly
- No shell injection possible through any user input
- base64 encoding is pure Rust, no shell dependency
