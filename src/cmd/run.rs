use crate::types::*;
use crate::lifecycle;
use crate::container;
use crate::scripts;
use std::path::PathBuf;
use colored::Colorize;

pub(crate) async fn cmd_run(
    name: &SessionName,
    prompt: &str,
    dockerfile: Option<PathBuf>,
) -> anyhow::Result<()> {
    eprintln!("{}", format!("→ Running prompt in session '{}'", name).as_str().blue());
    eprintln!("  Prompt: {}", if prompt.len() > 60 { format!("{}...", &prompt[..60]) } else { prompt.to_string() });

    let lc = lifecycle::Lifecycle::new()?;

    // Step 1: Resolve image
    let image = if let Some(ref df) = dockerfile {
        let df_path = if df.is_dir() {
            let candidate = df.join("Dockerfile");
            if candidate.exists() { candidate } else {
                anyhow::bail!("No Dockerfile found in {}", df.display());
            }
        } else {
            df.clone()
        };
        let image_name = format!("claude-dev-{}", name);
        let image_ref = ImageRef::new(&image_name);
        eprintln!("  Building image: {}", image_name);
        lc.build_image(&image_ref, &df_path, &df_path.parent().unwrap_or(&PathBuf::from("."))).await?;
        image_ref
    } else {
        ImageRef::new("ghcr.io/hypermemetic/claude-container:latest")
    };

    // Step 2: Verified pipeline
    let docker = container::verify_docker(&lc).await?;
    let verified_image = container::verify_image(&lc, &docker, &image).await?;
    for tool in verified_image.validation.missing_optional() {
        eprintln!("  {} {} (optional)", "⚠".yellow(), tool);
    }
    let volumes = container::verify_volumes(&lc, &docker, name).await?;

    // Token
    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .or_else(|_| {
            let token_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".config/claude-container/token");
            std::fs::read_to_string(&token_file)
        })
        .map_err(|_| anyhow::anyhow!("No auth token found. Set CLAUDE_CODE_OAUTH_TOKEN or create ~/.config/claude-container/token"))?;
    let verified_token = container::verify_token(&lc, token.trim())?;

    // Materialize embedded scripts
    let script_dir = scripts::materialize()?;

    // Plan target
    let target = container::plan_target(&lc, &docker, name, &verified_image, &script_dir).await
        .or_else(|e| {
            if let ContainerError::ContainerRunning(ref _ctr) = e {
                Err(e)
            } else {
                Err(e)
            }
        })?;

    // Build LaunchReady
    let ready = crate::types::verified::LaunchReady {
        docker,
        image: verified_image,
        volumes,
        token: verified_token,
        container: target,
    };

    // Step 3: Run headless
    eprintln!();
    let output = container::run_headless(&lc, ready, name, &script_dir, prompt).await?;

    // Step 4: Print captured output
    if !output.is_empty() {
        println!("{}", output);
    }

    eprintln!();
    eprintln!("{}", "→ Run complete.".green());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires Docker
    async fn cmd_run_is_callable() {
        let name = SessionName::new("test-nonexistent-run");
        let result = cmd_run(&name, "echo hello", None).await;
        // Will fail because session doesn't exist
        assert!(result.is_err());
    }
}
