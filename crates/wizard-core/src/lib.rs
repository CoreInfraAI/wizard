use anyhow::ensure;

pub mod agents;
mod config_files;
mod platform;
pub mod settings;

pub fn validate_token(token: &str) -> anyhow::Result<()> {
    // TODO: fully validate the token with CoreInfra Hub, not just its format.
    ensure!(
        token.is_empty()
            || (token.len() >= 20 && token.bytes().all(|byte| byte.is_ascii_graphic())),
        "CoreInfra token must contain at least 20 printable ASCII characters without spaces"
    );
    Ok(())
}
