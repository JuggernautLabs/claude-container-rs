//! Error types — every failure mode is a typed variant

use std::path::PathBuf;
use super::{ContainerName, ImageRef, SessionName, VolumeName};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ContainerError {
    // --- Docker errors ---
    #[error("Docker not available: {0}")]
    DockerUnavailable(String),

    #[error("Image not found: {0}")]
    ImageNotFound(ImageRef),

    #[error("Image invalid: {image} — missing: {missing:?}")]
    ImageInvalid {
        image: ImageRef,
        missing: Vec<String>,
    },

    #[error("Container not found: {0}")]
    ContainerNotFound(ContainerName),

    #[error("Container is running: {0} — stop it first")]
    ContainerRunning(ContainerName),

    #[error("Container stale: {name} — {reasons:?}")]
    ContainerStale {
        name: ContainerName,
        reasons: Vec<String>,
    },

    // --- Session errors ---
    #[error("Session not found: {0}")]
    SessionNotFound(SessionName),

    #[error("Session already exists: {0} — use --force to recreate")]
    SessionExists(SessionName),

    #[error("Volume missing: {0}")]
    VolumeMissing(VolumeName),

    // --- Token errors ---
    #[error("No auth token found")]
    NoToken,

    #[error("Token file corrupted: {0}")]
    TokenCorrupted(PathBuf),

    // --- Git errors ---
    #[error("Not a git repo: {0}")]
    NotAGitRepo(PathBuf),

    #[error("Branch not found: {branch} in {repo}")]
    BranchNotFound { repo: String, branch: String },

    #[error("Merge conflict in {repo}: {files:?}")]
    MergeConflict { repo: String, files: Vec<String> },

    #[error("Bundle creation failed for {repo}: {reason}")]
    BundleFailed { repo: String, reason: String },

    #[error("Fetch failed for {repo}: {reason}")]
    FetchFailed { repo: String, reason: String },

    #[error("Branch creation failed for {repo}: {reason}")]
    BranchCreateFailed { repo: String, reason: String },

    #[error("Injection failed for {repo}: {reason}")]
    InjectionFailed { repo: String, reason: String },

    // --- Config errors ---
    #[error("Config not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error("Config parse error: {0}")]
    ConfigParse(String),

    // --- Safety errors ---
    #[error("Destructive operation blocked: {0} — use --force or run interactively")]
    DestructiveBlocked(String),

    #[error("Non-interactive mode: {0}")]
    NonInteractive(String),

    // --- Wrapped errors ---
    #[error("Docker API error: {0}")]
    Docker(#[from] bollard::errors::Error),

    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
