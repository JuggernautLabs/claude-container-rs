//! Embedded container scripts — compiled into the binary via include_str!.
//!
//! These are the bash scripts that run inside the container:
//!   cc-entrypoint      — root phase: token, user creation, volume ownership
//!   cc-developer-setup — developer phase: .claude.json, trust, gitconfig, launch
//!   cc-agent-run       — agent wrapper: HEAD snapshot, run claude, write .agent-result
//!
//! At runtime, `materialize()` writes them to a cache directory so Docker
//! can bind-mount them into containers.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const ENTRYPOINT: &str = include_str!("../scripts/container/cc-entrypoint");
const DEVELOPER_SETUP: &str = include_str!("../scripts/container/cc-developer-setup");
const AGENT_RUN: &str = include_str!("../scripts/container/cc-agent-run");

pub const SCRIPTS: &[(&str, &str)] = &[
    ("cc-entrypoint", ENTRYPOINT),
    ("cc-developer-setup", DEVELOPER_SETUP),
    ("cc-agent-run", AGENT_RUN),
];

/// Write all embedded scripts to a stable cache directory.
/// Returns the directory path for use as Docker bind-mount source.
///
/// Uses `~/.cache/gitvm/scripts/` — deterministic so the staleness
/// check can compare mount paths across process invocations.
/// Scripts are overwritten every time (handles binary upgrades).
pub fn materialize() -> anyhow::Result<PathBuf> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;

    for (name, content) in SCRIPTS {
        let path = dir.join(name);
        std::fs::write(&path, content)?;
        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(dir)
}

/// The stable directory where scripts are materialized.
pub fn cache_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".cache/gitvm/scripts")
}

/// Check whether scripts on disk match what's embedded in the binary.
pub fn scripts_are_current(dir: &Path) -> bool {
    SCRIPTS.iter().all(|(name, content)| {
        let path = dir.join(name);
        std::fs::read_to_string(&path)
            .map(|on_disk| on_disk == *content)
            .unwrap_or(false)
    })
}
