use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use reqwest::{blocking::Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    thread,
    time::{Duration, Instant},
};

const VERIFIER_BYTES: usize = 32;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_POLL_INTERVAL: u64 = 10;

pub struct TelegramAuthOptions {
    pub endpoint: String,
    pub no_open: bool,
}

#[derive(Serialize)]
struct CreatePairingRequest<'a> {
    code_challenge: &'a str,
    client: &'static str,
    client_version: &'static str,
}

#[derive(Deserialize)]
struct CreatePairingResponse {
    session_id: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Serialize)]
struct ExchangeRequest<'a> {
    code_verifier: &'a str,
}

#[derive(Deserialize, Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_at: u64,
    telegram_user_id: i64,
}

#[derive(Deserialize)]
struct ApiError {
    error: String,
}

#[derive(Serialize)]
struct CredentialFile<'a> {
    version: u8,
    endpoint: &'a str,
    token_type: &'a str,
    access_token: &'a str,
    expires_at: u64,
    telegram_user_id: i64,
}

pub fn authorize_telegram(options: TelegramAuthOptions) -> Result<()> {
    let endpoint = normalize_endpoint(&options.endpoint)?;
    let verifier = random_token(VERIFIER_BYTES);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("infraCLI/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build authorization client")?;

    let pairing = client
        .post(format!("{endpoint}/v1/pairings"))
        .json(&CreatePairingRequest {
            code_challenge: &challenge,
            client: "infraCLI",
            client_version: env!("CARGO_PKG_VERSION"),
        })
        .send()
        .context("create Telegram pairing session")?
        .error_for_status()
        .context("infraBot rejected the pairing request")?
        .json::<CreatePairingResponse>()
        .context("decode pairing response")?;

    println!("authorize infraCLI in Telegram:");
    println!("{}", pairing.verification_uri_complete);

    if !options.no_open && !open_uri(&pairing.verification_uri_complete) {
        println!("open the link manually if it did not open automatically");
    }

    println!("waiting for confirmation...");
    let token = poll_for_token(&client, &endpoint, &pairing, &verifier)?;
    let path = write_credentials(&endpoint, &token)?;

    println!("authorized as Telegram user {}", token.telegram_user_id);
    println!("credentials stored at {}", path.display());
    Ok(())
}

fn poll_for_token(
    client: &Client,
    endpoint: &str,
    pairing: &CreatePairingResponse,
    verifier: &str,
) -> Result<TokenResponse> {
    let started = Instant::now();
    let deadline = Duration::from_secs(pairing.expires_in);
    let mut interval = pairing.interval.max(1);
    let url = format!("{endpoint}/v1/pairings/{}/exchange", pairing.session_id);

    while started.elapsed() < deadline {
        thread::sleep(Duration::from_secs(interval));
        let response = client
            .post(&url)
            .json(&ExchangeRequest {
                code_verifier: verifier,
            })
            .send()
            .context("poll Telegram pairing session")?;

        match response.status() {
            StatusCode::OK => {
                let token = response
                    .json::<TokenResponse>()
                    .context("decode authorization token")?;
                if token.token_type != "Bearer" || token.access_token.is_empty() {
                    bail!("infraBot returned an invalid authorization token");
                }
                return Ok(token);
            }
            StatusCode::ACCEPTED => continue,
            StatusCode::TOO_MANY_REQUESTS => {
                interval = (interval + 2).min(MAX_POLL_INTERVAL);
            }
            StatusCode::GONE => bail!("Telegram authorization expired; run the command again"),
            status => {
                let error = response.json::<ApiError>().ok();
                let message = error
                    .map(|value| value.error)
                    .unwrap_or_else(|| format!("HTTP {status}"));
                bail!("Telegram authorization failed: {message}");
            }
        }
    }

    bail!("Telegram authorization expired; run the command again")
}

fn normalize_endpoint(value: &str) -> Result<String> {
    let endpoint = value.trim().trim_end_matches('/').to_owned();
    let secure = endpoint.starts_with("https://");
    let local = endpoint.starts_with("http://127.0.0.1")
        || endpoint.starts_with("http://localhost")
        || endpoint.starts_with("http://[::1]");

    if endpoint.is_empty() {
        bail!("infraBot endpoint is required");
    }
    if !secure && !local {
        bail!("infraBot endpoint must use HTTPS; HTTP is allowed only for localhost");
    }
    Ok(endpoint)
}

fn random_token(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn credentials_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("INFRA_CREDENTIALS_FILE") {
        return Ok(PathBuf::from(path));
    }
    if let Some(root) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(root).join("infra/credentials.json"));
    }
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".config/infra/credentials.json"));
    }
    bail!("cannot resolve credentials path; set INFRA_CREDENTIALS_FILE")
}

fn write_credentials(endpoint: &str, token: &TokenResponse) -> Result<PathBuf> {
    let path = credentials_path()?;
    let parent = path.parent().context("credentials path has no parent")?;
    create_private_directory(parent)?;

    let document = CredentialFile {
        version: 1,
        endpoint,
        token_type: &token.token_type,
        access_token: &token.access_token,
        expires_at: token.expires_at,
        telegram_user_id: token.telegram_user_id,
    };
    let content = serde_json::to_vec_pretty(&document).context("encode credentials")?;
    let temporary = parent.join(format!(".credentials-{}.tmp", std::process::id()));

    write_private_file(&temporary, &content)?;
    fs::rename(&temporary, &path).with_context(|| format!("replace {}", path.display()))?;
    Ok(path)
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure {}", path.display()))
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))
}

#[cfg(unix)]
fn write_private_file(path: &Path, content: &[u8]) -> Result<()> {
    use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, content: &[u8]) -> Result<()> {
    let mut file = fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn open_uri(uri: &str) -> bool {
    #[cfg(target_os = "macos")]
    let result = ProcessCommand::new("open").arg(uri).status();

    #[cfg(target_os = "windows")]
    let result = ProcessCommand::new("cmd")
        .args(["/C", "start", "", uri])
        .status();

    #[cfg(all(unix, not(target_os = "macos")))]
    let result = ProcessCommand::new("xdg-open").arg(uri).status();

    #[cfg(not(any(unix, target_os = "windows")))]
    let result: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "opening URLs is unsupported",
    ));

    result.is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_https_outside_localhost() {
        assert!(normalize_endpoint("https://bot.example").is_ok());
        assert!(normalize_endpoint("http://localhost:8787").is_ok());
        assert!(normalize_endpoint("http://127.0.0.1:8787").is_ok());
        assert!(normalize_endpoint("http://bot.example").is_err());
    }

    #[test]
    fn challenge_is_sha256_base64url() {
        let verifier = random_token(VERIFIER_BYTES);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
    }
}
