use std::process::Command;

#[test]
fn credential_helper_rejects_bad_args_and_zero_arg_helper_context() {
    let binary = env!("CARGO_BIN_EXE_wizard");

    let missing_args = Command::new(binary)
        .arg(wizard_core::agents::claude_desktop::credential::ARG)
        .output()
        .expect("spawn wizard with missing credential args");
    assert!(!missing_args.status.success());
    assert!(missing_args.stdout.is_empty());

    let no_arg_helper = Command::new(binary)
        .env("CLAUDE_HELPER_CONTEXT", "startup")
        .output()
        .expect("spawn wizard with CLAUDE_HELPER_CONTEXT");
    assert!(!no_arg_helper.status.success());
    assert!(no_arg_helper.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&no_arg_helper.stderr).contains("2.110.0"),
        "expected version guidance on stderr"
    );
}

#[cfg(unix)]
#[test]
fn credential_helper_prints_exact_token_for_active_profile() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let binary = env!("CARGO_BIN_EXE_wizard");
    let home = tempfile::tempdir()?;
    #[cfg(target_os = "macos")]
    let app_root = home.path().join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let app_root = home.path().join("config");

    let id = "11111111-1111-4111-8111-111111111111";
    let token = "subprocess-hub-token";
    let digest = wizard_core::media::fingerprint(token);
    let config_library = app_root.join("Claude-3p/configLibrary");
    let key_dir = app_root
        .join(wizard_settings::paths::IDENTIFIER)
        .join("claude-desktop-credentials");
    std::fs::create_dir_all(&config_library)?;
    std::fs::create_dir_all(&key_dir)?;
    std::fs::write(
        config_library.join("_meta.json"),
        serde_json::json!({"appliedId": id, "entries": [{"id": id, "name": "CoreInfra"}]})
            .to_string(),
    )?;
    std::fs::write(
        app_root.join("Claude-3p/claude_desktop_config.json"),
        r#"{"deploymentMode":"3p"}"#,
    )?;
    std::fs::write(
        config_library.join(format!("{id}.json")),
        serde_json::json!({
            "inferenceProvider": "gateway",
            "inferenceGatewayBaseUrl": "https://hub.coreinfra.ai/claude-desktop/api",
            "inferenceCredentialKind": "helper-script",
            "inferenceCredentialHelper": binary,
            "inferenceCredentialHelperArgs": [
                wizard_core::agents::claude_desktop::credential::ARG,
                id,
                digest,
            ],
        })
        .to_string(),
    )?;
    let key_path = key_dir.join(format!("{id}.key"));
    std::fs::write(
        &key_path,
        serde_json::json!({"helper": binary, "token": token}).to_string(),
    )?;
    std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;

    let mut command = Command::new(binary);
    command
        .args([
            wizard_core::agents::claude_desktop::credential::ARG,
            id,
            &digest,
        ])
        .env("HOME", home.path());
    #[cfg(not(target_os = "macos"))]
    command.env("XDG_CONFIG_HOME", &app_root);

    let output = command.output()?;
    assert!(
        output.status.success(),
        "helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout)?, format!("{token}\n"));

    let mut stale = Command::new(binary);
    stale
        .args([
            wizard_core::agents::claude_desktop::credential::ARG,
            id,
            "wrong-digest",
        ])
        .env("HOME", home.path());
    #[cfg(not(target_os = "macos"))]
    stale.env("XDG_CONFIG_HOME", &app_root);
    let stale_output = stale.output()?;
    assert!(!stale_output.status.success());
    assert!(stale_output.stdout.is_empty());
    Ok(())
}
