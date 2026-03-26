use crate::types::*;
use crate::lifecycle;
use crate::render;

pub(crate) async fn cmd_validate_image(image: &str, force: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    let image_ref = ImageRef::new(image);

    if force {
        // Evict cached result so validate_image() re-runs from scratch
        let inspect = lc.docker_client()
            .inspect_image(image_ref.as_str())
            .await
            .map_err(|_| anyhow::anyhow!("Image not found: {}", image))?;
        let image_id = inspect.id.unwrap_or_default();
        if let Some(cache_path) = lifecycle::validation_cache_path(&image_id) {
            let _ = std::fs::remove_file(cache_path);
        }
    }

    let validation = lc.validate_image(&image_ref).await?;
    render::image_validation(&validation);
    if !validation.is_valid() {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires Docker
    async fn validate_nonexistent_image_errors() {
        let result = cmd_validate_image("nonexistent-image-abc123", false).await;
        assert!(result.is_err());
    }
}
