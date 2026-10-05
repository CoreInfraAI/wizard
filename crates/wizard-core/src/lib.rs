use std::sync::LazyLock;

use anyhow::ensure;
use regex::Regex;

pub mod agents;
mod config_files;
mod platform;
pub mod settings;

pub fn validate_token(token: &str) -> anyhow::Result<()> {
    static TOKEN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\Ask-ci(?:-[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+|2-[A-Za-z0-9_-]{37}[AQgw])\.[A-Za-z0-9_-]{85}[AQgw]\z")
            .expect("invalid CoreInfra token regex")
    });

    ensure!(
        token.is_empty() || TOKEN_REGEX.is_match(token),
        "Invalid CoreInfra token format"
    );
    Ok(())
}
