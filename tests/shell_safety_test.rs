//! GS-9: Shell safety tests — command escaping, base64, config writing.

// We test the public helpers via the lib crate.
use gitvm::shell_safety;

#[test]
fn exec_single_arg_uses_bash_c() {
    // Single arg → bash -c "arg"
    let cmd = shell_safety::build_exec_cmd(&["ls -la /tmp".to_string()]);
    assert_eq!(cmd, vec!["bash", "-c", "ls -la /tmp"]);
}

#[test]
fn exec_multiple_args_direct() {
    // Multiple args → pass directly (no shell wrapping)
    let cmd = shell_safety::build_exec_cmd(&[
        "echo".to_string(),
        "hello world".to_string(),
    ]);
    assert_eq!(cmd, vec!["echo", "hello world"]);
}

#[test]
fn exec_handles_special_chars() {
    // Multiple args with special chars → no shell expansion
    let cmd = shell_safety::build_exec_cmd(&[
        "echo".to_string(),
        "it's a \"test\" $HOME".to_string(),
    ]);
    assert_eq!(cmd, vec!["echo", "it's a \"test\" $HOME"]);
}

#[test]
fn exec_empty_gives_bash() {
    let cmd = shell_safety::build_exec_cmd(&[]);
    assert_eq!(cmd, vec!["bash"]);
}

#[test]
fn base64_roundtrip_simple() {
    let input = "hello world";
    let encoded = shell_safety::base64_encode(input);
    let decoded = shell_safety::base64_decode(&encoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn base64_roundtrip_quotes_and_special() {
    let input = r#"it's a "test" with $HOME and `backticks`"#;
    let encoded = shell_safety::base64_encode(input);
    let decoded = shell_safety::base64_decode(&encoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn base64_roundtrip_newlines() {
    let input = "line1\nline2\nline3\n";
    let encoded = shell_safety::base64_encode(input);
    let decoded = shell_safety::base64_decode(&encoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn base64_roundtrip_unicode() {
    let input = "hello 世界 🌍 café";
    let encoded = shell_safety::base64_encode(input);
    let decoded = shell_safety::base64_decode(&encoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn base64_roundtrip_empty() {
    let input = "";
    let encoded = shell_safety::base64_encode(input);
    let decoded = shell_safety::base64_decode(&encoded).unwrap();
    assert_eq!(decoded, input);
}

#[test]
fn base64_encode_is_pure_rust() {
    // Verify we get a valid base64 string (no shell command involved)
    let encoded = shell_safety::base64_encode("test");
    // base64("test") = "dGVzdA=="
    assert_eq!(encoded, "dGVzdA==");
}

#[test]
fn write_config_script_roundtrips_special_yaml() {
    // Config with special characters in repo names and paths
    let yaml = r#"version: "1"
projects:
  "it's-a-repo":
    path: "/path/with spaces/and 'quotes'"
    extract: true
    main: false
  "repo:with:colons":
    path: "/normal/path"
    extract: true
    main: false
"#;
    let script = shell_safety::write_config_script(yaml);
    // The script should use base64 encoding, not raw shell quoting
    assert!(script.contains("base64"), "script should use base64 encoding: {}", script);
    assert!(!script.contains("echo '"), "script should NOT use shell echo with single quotes: {}", script);
    // Verify the b64 payload decodes back to original
    let b64_part = script.split("echo ").nth(1).unwrap().split(" |").next().unwrap().trim();
    let decoded = shell_safety::base64_decode(b64_part).unwrap();
    assert_eq!(decoded, yaml);
}
