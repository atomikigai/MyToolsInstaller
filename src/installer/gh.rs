use anyhow::{Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::detect::Distro;
use crate::util::run;

// Public OAuth client ID of the official `gh` CLI app. Same value gh itself
// uses for its device flow against github.com.
const GH_CLIENT_ID: &str = "178c6fc778ccc68e1d6a";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const SCOPES: &str = "repo,read:org,gist,workflow";

pub fn install(distro: Distro) -> Result<()> {
    match distro {
        Distro::Arch => {
            println!("→ installing GitHub CLI via pacman");
            run("sudo", &["pacman", "-S", "--needed", "--noconfirm", "github-cli"])?;
        }
        Distro::Other => {
            anyhow::bail!("automatic gh install only supported on Arch/CachyOS for now")
        }
    }

    println!();
    println!("→ starting GitHub device-flow login (no prompts — just open the URL)");
    device_login()?;
    Ok(())
}

/// Drive the GitHub OAuth device flow directly via curl, then hand the
/// resulting token to `gh auth login --with-token`. No interactive `gh`
/// prompts are involved, so the installer never gets stuck on Y/n.
pub fn device_login() -> Result<()> {
    let DeviceCode { device_code, user_code, verification_uri, interval, expires_in }
        = request_device_code()?;

    println!();
    println!("════════════════════════════════════════════════════════════════");
    println!("  Open this URL in your browser:");
    println!("      {verification_uri}");
    println!();
    println!("  And enter this code:");
    println!("      {user_code}");
    println!();
    println!("  Waiting for you to authorize… (expires in {expires_in}s)");
    println!("════════════════════════════════════════════════════════════════");
    println!();

    let token = poll_for_token(&device_code, interval, expires_in)?;
    save_token_via_gh(&token)?;

    // Register gh as git credential helper — also non-interactive.
    let _ = Command::new("gh").args(["auth", "setup-git"]).status();

    println!("✓ gh authenticated and git credential helper configured");
    Ok(())
}

struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: u64,
    expires_in: u64,
}

fn request_device_code() -> Result<DeviceCode> {
    let body = format!("client_id={GH_CLIENT_ID}&scope={SCOPES}");
    let out = Command::new("curl")
        .args([
            "-sS",
            "-X", "POST",
            "-H", "Accept: application/json",
            "-H", "Content-Type: application/x-www-form-urlencoded",
            "-d", &body,
            DEVICE_CODE_URL,
        ])
        .output()
        .context("failed to run curl for device code")?;

    if !out.status.success() {
        anyhow::bail!("curl failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }

    let json = String::from_utf8_lossy(&out.stdout);
    Ok(DeviceCode {
        device_code:      json_str(&json, "device_code").context("missing device_code")?,
        user_code:        json_str(&json, "user_code").context("missing user_code")?,
        verification_uri: json_str(&json, "verification_uri").context("missing verification_uri")?,
        interval:         json_num(&json, "interval").unwrap_or(5),
        expires_in:       json_num(&json, "expires_in").unwrap_or(900),
    })
}

fn poll_for_token(device_code: &str, interval: u64, expires_in: u64) -> Result<String> {
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    let mut wait = Duration::from_secs(interval.max(1));

    loop {
        thread::sleep(wait);
        if Instant::now() > deadline {
            anyhow::bail!("device code expired before authorization");
        }

        let body = format!(
            "client_id={GH_CLIENT_ID}&device_code={device_code}\
             &grant_type=urn:ietf:params:oauth:grant-type:device_code"
        );
        let out = Command::new("curl")
            .args([
                "-sS",
                "-X", "POST",
                "-H", "Accept: application/json",
                "-H", "Content-Type: application/x-www-form-urlencoded",
                "-d", &body,
                ACCESS_TOKEN_URL,
            ])
            .output()
            .context("failed to poll for token")?;

        if !out.status.success() {
            anyhow::bail!("curl failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }

        let json = String::from_utf8_lossy(&out.stdout);
        if let Some(token) = json_str(&json, "access_token") {
            return Ok(token);
        }
        match json_str(&json, "error").as_deref() {
            Some("authorization_pending") => { /* keep waiting */ }
            Some("slow_down") => { wait += Duration::from_secs(5); }
            Some("expired_token") => anyhow::bail!("device code expired"),
            Some("access_denied") => anyhow::bail!("authorization denied"),
            Some(other) => anyhow::bail!("oauth error: {other}"),
            None => anyhow::bail!("unexpected response: {}", json.trim()),
        }
    }
}

fn save_token_via_gh(token: &str) -> Result<()> {
    let mut child = Command::new("gh")
        .args([
            "auth", "login",
            "--hostname", "github.com",
            "--git-protocol", "https",
            "--with-token",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn `gh auth login`")?;
    {
        let stdin = child.stdin.as_mut().context("no stdin on gh")?;
        stdin.write_all(token.as_bytes())?;
        stdin.write_all(b"\n")?;
    }
    let out = child.wait_with_output().context("waiting for gh")?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Tiny JSON string-value extractor. The GitHub responses we hit are flat
/// objects with snake_case keys, so a regex-free scan is enough.
fn json_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let after = after.strip_prefix('"')?;
    let end = after.find('"')?;
    Some(after[..end].to_string())
}

fn json_num(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
    after[..end].parse().ok()
}
