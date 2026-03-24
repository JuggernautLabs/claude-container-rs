use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use git_sandbox::lifecycle::{validation_cache_path, hash_string, VALIDATION_CACHE_TTL};
use git_sandbox::types::{ImageRef, ImageValidation, BinaryCheck};

/// Helper: build a minimal valid cache file content
fn cache_content() -> String {
    "claude:critical:ok\ngit:critical:ok\ncurl:optional:ok\n".to_string()
}

/// Helper: write a cache file and set its mtime to `age` ago
fn write_cache_with_age(image_id: &str, age: Duration) -> PathBuf {
    let path = validation_cache_path(image_id).expect("cache path");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create cache dir");
    }
    std::fs::write(&path, cache_content()).expect("write cache");

    // Set file mtime to `age` ago using filetime crate (or just touch it)
    // We'll use a raw approach: write, then use std::fs::File::set_modified (nightly)
    // Instead, use filetime via std::process::Command or set_file_times
    let target_time = SystemTime::now() - age;
    let file = std::fs::File::options().write(true).open(&path).expect("open for mtime");
    file.set_modified(target_time).expect("set mtime");
    path
}

/// Helper: clean up a cache file
fn cleanup_cache(image_id: &str) {
    if let Some(path) = validation_cache_path(image_id) {
        let _ = std::fs::remove_file(path);
    }
}

// ============================================================================
// TTL tests
// ============================================================================

#[test]
fn cache_expires_after_ttl() {
    let image_id = "sha256:expired_test_image_id_0001";
    let _path = write_cache_with_age(image_id, VALIDATION_CACHE_TTL + Duration::from_secs(3600));

    // load_validation_cache should return None for expired cache
    let image_ref = ImageRef::new("test:latest");
    let result = git_sandbox::lifecycle::load_validation_cache_standalone(image_id, &image_ref);
    assert!(result.is_none(), "Expired cache should return None");

    cleanup_cache(image_id);
}

#[test]
fn cache_valid_within_ttl() {
    let image_id = "sha256:fresh_test_image_id_0002";
    let _path = write_cache_with_age(image_id, Duration::from_secs(3600)); // 1 hour ago

    let image_ref = ImageRef::new("test:latest");
    let result = git_sandbox::lifecycle::load_validation_cache_standalone(image_id, &image_ref);
    assert!(result.is_some(), "Fresh cache should return Some");

    let validation = result.unwrap();
    assert_eq!(validation.image.as_str(), "test:latest");
    assert!(!validation.critical.is_empty(), "Should have critical binaries");

    cleanup_cache(image_id);
}

#[test]
fn cache_keyed_by_image_id_not_name() {
    let image_id_a = "sha256:image_a_for_keying_test_0003";
    let image_id_b = "sha256:image_b_for_keying_test_0004";

    // Write cache for image_id_a only
    let _path = write_cache_with_age(image_id_a, Duration::from_secs(60));

    let image_ref = ImageRef::new("myimage:latest");

    // Same image name, but different ID (image_id_b) → cache miss
    let result_b = git_sandbox::lifecycle::load_validation_cache_standalone(image_id_b, &image_ref);
    assert!(result_b.is_none(), "Different image ID should be a cache miss");

    // Original image ID → cache hit
    let result_a = git_sandbox::lifecycle::load_validation_cache_standalone(image_id_a, &image_ref);
    assert!(result_a.is_some(), "Original image ID should be a cache hit");

    cleanup_cache(image_id_a);
    cleanup_cache(image_id_b);
}

#[test]
fn cache_missing_file_returns_none() {
    let image_id = "sha256:nonexistent_image_id_0005";
    // Make sure no file exists
    cleanup_cache(image_id);

    let image_ref = ImageRef::new("test:latest");
    let result = git_sandbox::lifecycle::load_validation_cache_standalone(image_id, &image_ref);
    assert!(result.is_none(), "Missing cache file should return None");
}

#[test]
fn cache_at_exact_ttl_boundary_expires() {
    let image_id = "sha256:boundary_test_image_id_0006";
    // Write cache at exactly the TTL boundary (should be expired)
    let _path = write_cache_with_age(image_id, VALIDATION_CACHE_TTL);

    let image_ref = ImageRef::new("test:latest");
    let result = git_sandbox::lifecycle::load_validation_cache_standalone(image_id, &image_ref);
    assert!(result.is_none(), "Cache at exact TTL boundary should expire");

    cleanup_cache(image_id);
}

#[test]
fn hash_string_is_deterministic() {
    let h1 = hash_string("sha256:abc123");
    let h2 = hash_string("sha256:abc123");
    assert_eq!(h1, h2);

    let h3 = hash_string("sha256:different");
    assert_ne!(h1, h3);
}
