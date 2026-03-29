//! Image validation types

use super::ImageRef;

/// Result of validating a Docker image
#[derive(Debug, Clone)]
pub struct ImageValidation {
    pub image: ImageRef,
    pub critical: Vec<BinaryCheck>,
    pub optional: Vec<BinaryCheck>,
}

#[derive(Debug, Clone)]
pub struct BinaryCheck {
    pub name: String,
    pub present: bool,
    pub functional: bool, // e.g., gosu can actually drop privileges
}

impl ImageValidation {
    pub fn is_valid(&self) -> bool {
        self.critical.iter().all(|c| c.present && c.functional)
    }

    pub fn missing_critical(&self) -> Vec<&str> {
        self.critical.iter()
            .filter(|c| !c.present || !c.functional)
            .map(|c| c.name.as_str())
            .collect()
    }

    pub fn missing_optional(&self) -> Vec<&str> {
        self.optional.iter()
            .filter(|c| !c.present)
            .map(|c| c.name.as_str())
            .collect()
    }
}

/// Required binaries in the image (no fallback)
pub const CRITICAL_BINARIES: &[&str] = &["git", "bash"];

/// Optional binaries (warning only — entrypoint handles absence gracefully)
pub const OPTIONAL_BINARIES: &[&str] = &["claude", "gosu", "sudo", "python3", "docker"];
