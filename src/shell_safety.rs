//! Shell safety helpers — pure-Rust base64, safe exec arg building, safe config writing.
//!
//! GS-9: Eliminates shell injection vectors by:
//! - Using the `base64` crate instead of shelling out to `/usr/bin/base64`
//! - Building docker exec commands without joining args through a shell
//! - Writing config YAML via base64-encoded pipe instead of shell quoting

use base64::Engine;

/// Build the command vector for `docker exec`.
///
/// - Empty args → interactive bash shell
/// - Single arg → `bash -c "<arg>"` (treats it as a shell command string)
/// - Multiple args → pass directly to exec (no shell wrapping, no injection)
pub fn build_exec_cmd(args: &[String]) -> Vec<String> {
    if args.is_empty() {
        vec!["bash".to_string()]
    } else if args.len() == 1 {
        vec!["bash".to_string(), "-c".to_string(), args[0].clone()]
    } else {
        args.to_vec()
    }
}

/// Base64-encode a string using pure Rust (no shell dependency).
pub fn base64_encode(input: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(input.as_bytes())
}

/// Base64-decode a string using pure Rust.
pub fn base64_decode(input: &str) -> Result<String, base64::DecodeError> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(input)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Build a shell script that writes YAML config into a container via base64 decoding.
///
/// Instead of `echo '<escaped>' > file` (fragile with quotes/special chars),
/// we do `echo <base64> | base64 -d > file` which is safe for any content.
pub fn write_config_script(yaml: &str) -> String {
    let b64 = base64_encode(yaml);
    format!("echo {} | base64 -d > /session/.claude-projects.yml", b64)
}
