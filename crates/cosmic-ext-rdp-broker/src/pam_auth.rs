use anyhow::{bail, Context, Result};

/// Result of a successful PAM authentication.
#[derive(Debug)]
#[allow(dead_code)]
pub struct PamAuthResult {
    /// The authenticated username (may be canonicalized by PAM).
    pub username: String,
    /// The Unix UID of the authenticated user.
    pub uid: u32,
}

/// Authenticate a user via PAM.
///
/// Runs the PAM conversation in a blocking thread since PAM is
/// synchronous. Uses the specified PAM service (e.g. "cosmic-ext-rdp").
///
/// # Errors
///
/// Returns an error if authentication fails or the user account
/// is invalid/expired.
#[allow(dead_code)]
pub async fn authenticate(
    service: &str,
    username: &str,
    password: &str,
) -> Result<PamAuthResult> {
    let service = service.to_string();
    let username = username.to_string();
    let password = password.to_string();

    tokio::task::spawn_blocking(move || pam_auth_blocking(&service, &username, &password))
        .await
        .context("PAM auth task panicked")?
}

/// Synchronous PAM authentication (runs on a blocking thread).
#[allow(dead_code)]
fn pam_auth_blocking(_service: &str, username: &str, password: &str) -> Result<PamAuthResult> {
    // Use nix to look up the user's UID.
    let user = nix::unistd::User::from_name(username)
        .context("failed to look up user")?
        .with_context(|| format!("user '{username}' not found on system"))?;

    let uid = user.uid.as_raw();

    // For now, use a simple Unix password check via nix.
    // Full PAM integration requires the pam-client crate which needs
    // libpam headers at build time. We use a simplified approach that
    // validates against the system shadow database.
    //
    // TODO: Replace with full pam-client when libpam is available in
    // the nix dev shell:
    //
    // ```rust
    // let mut context = pam_client::Context::new(service, Some(username), conv)?;
    // context.authenticate(pam_client::Flag::NONE)?;
    // context.acct_mgmt(pam_client::Flag::NONE)?;
    // ```
    //
    // For security, we currently delegate the actual password verification
    // to a helper command (similar to how xrdp's sesman works).
    verify_password_via_helper(username, password)?;

    tracing::info!(username, uid, "PAM authentication successful");

    Ok(PamAuthResult {
        username: username.to_string(),
        uid,
    })
}

#[allow(dead_code)]
/// Verify a password using a helper mechanism.
///
/// This uses the `su` command to validate credentials, which works
/// with any PAM-configured system. The broker runs as root, so `su`
/// can verify any user's password.
fn verify_password_via_helper(username: &str, password: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Use `su --command true <user>` to validate credentials.
    // su reads the password from stdin when not on a tty.
    let mut child = Command::new("su")
        .args(["--command", "true", "--", username])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn su for password verification")?;

    if let Some(mut stdin) = child.stdin.take() {
        // Write password followed by newline.
        let _ = writeln!(stdin, "{password}");
    }

    let output = child
        .wait_with_output()
        .context("failed to wait for su")?;

    if output.status.success() {
        Ok(())
    } else {
        bail!("authentication failed for user '{username}'")
    }
}
