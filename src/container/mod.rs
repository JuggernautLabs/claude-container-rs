//! Container launch — the verified pipeline.
//!
//! Each step produces a Verified proof. The next step requires the proof.
//! You can't skip steps — the types won't let you.
//!
//! ```
//! let docker   = verify_docker(&lc).await?;                    // Verified<DockerAvailable>
//! let image    = verify_image(&lc, &docker, &image_ref).await?; // Verified<ValidImage>
//! let volumes  = verify_volumes(&lc, &docker, &name).await?;   // Verified<VolumesReady>
//! let token    = verify_token(&lc, &token_str).await?;          // Verified<TokenReady>
//! let target   = plan_target(&lc, &docker, &name, &image).await?; // LaunchTarget
//! let ready    = LaunchReady { docker, image, volumes, token, container: target };
//! launch(ready).await?;  // can only be called with LaunchReady
//! ```

use crate::types::*;
use crate::types::verified::*;
use crate::lifecycle::Lifecycle;
use std::path::Path;

/// Step 1: Verify Docker is available
pub async fn verify_docker(lc: &Lifecycle) -> Result<Verified<DockerAvailable>, ContainerError> {
    match lc.check_docker().await {
        docker::DockerState::Available { version } => {
            Ok(Verified::new_unchecked(DockerAvailable { version }))
        }
        docker::DockerState::NotRunning => {
            Err(ContainerError::DockerUnavailable("Docker daemon not running".into()))
        }
        docker::DockerState::NotInstalled => {
            Err(ContainerError::DockerUnavailable("Docker not installed".into()))
        }
    }
}

/// Step 2: Verify image meets the container protocol
pub async fn verify_image(
    lc: &Lifecycle,
    _docker: &Verified<DockerAvailable>,  // proof that docker is up
    image: &ImageRef,
) -> Result<Verified<ValidImage>, ContainerError> {
    let validation = lc.validate_image(image).await?;
    if !validation.is_valid() {
        let missing = validation.missing_critical().iter().map(|s| s.to_string()).collect();
        return Err(ContainerError::ImageInvalid {
            image: image.clone(),
            missing,
        });
    }
    let image_id = ImageId::new("TODO"); // would come from docker inspect
    Ok(Verified::new_unchecked(ValidImage {
        image: image.clone(),
        image_id,
        validation,
    }))
}

/// Step 3: Verify session volumes exist (create if needed)
pub async fn verify_volumes(
    lc: &Lifecycle,
    _docker: &Verified<DockerAvailable>,
    name: &SessionName,
) -> Result<Verified<VolumesReady>, ContainerError> {
    lc.create_volumes(name).await?;
    Ok(Verified::new_unchecked(VolumesReady {
        session: name.clone(),
    }))
}

/// Step 4: Verify token is available
pub fn verify_token(
    lc: &Lifecycle,
    token: &str,
) -> Result<Verified<TokenReady>, ContainerError> {
    let cache_dir = dirs::config_dir()
        .unwrap_or_default()
        .join("claude-container/cache");
    let mount = lc.inject_token(token, &cache_dir)?;
    Ok(Verified::new_unchecked(TokenReady { mount }))
}

/// Step 5: Determine launch target (requires docker + image verified)
pub async fn plan_target(
    lc: &Lifecycle,
    _docker: &Verified<DockerAvailable>,
    name: &SessionName,
    image: &Verified<ValidImage>,
    script_dir: &Path,
) -> Result<LaunchTarget, ContainerError> {
    let container_name = name.container_name();

    match lc.inspect_container(&container_name).await? {
        docker::ContainerState::NotFound { .. } => {
            Ok(LaunchTarget::Create)
        }
        docker::ContainerState::Running { .. } => {
            // Can't create — already running
            // Caller decides: attach or error
            Err(ContainerError::ContainerRunning(container_name))
        }
        docker::ContainerState::Stopped { info, .. } => {
            let check = lc.check_container(&container_name, &image.image, script_dir).await;
            match check {
                crate::lifecycle::ContainerCheck::Ready | crate::lifecycle::ContainerCheck::Resumable => {
                    Ok(LaunchTarget::Resume(Verified::new_unchecked(ContainerResumable {
                        name: container_name,
                    })))
                }
                crate::lifecycle::ContainerCheck::Stale { reasons } => {
                    // Need user confirmation to rebuild
                    // For now, return Rebuild without confirmation (TODO: interactive prompt)
                    Ok(LaunchTarget::Rebuild(Verified::new_unchecked(UserConfirmed {
                        description: format!("Rebuild container: {}", reasons.join(", ")),
                    })))
                }
                crate::lifecycle::ContainerCheck::Missing => {
                    Ok(LaunchTarget::Create)
                }
            }
        }
    }
}

/// Final step: launch the container. Requires ALL verifications passed.
/// This is the ONLY function that can create/start a container.
pub async fn launch(_ready: LaunchReady) -> Result<(), ContainerError> {
    // TODO: implement actual container creation + stdin/stdout attachment
    eprintln!("Launch ready — all verifications passed.");
    eprintln!("  Docker: {:?}", _ready.docker.version);
    eprintln!("  Image: {}", _ready.image.image);
    eprintln!("  Volumes: {}", _ready.volumes.session);
    match &_ready.container {
        LaunchTarget::Create => eprintln!("  Action: create new container"),
        LaunchTarget::Resume(c) => eprintln!("  Action: resume {}", c.name),
        LaunchTarget::Rebuild(c) => eprintln!("  Action: rebuild ({})", c.description),
    }
    Ok(())
}
