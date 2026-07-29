use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use reqwest::{StatusCode, Url, blocking::Client};
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
    pub source: String,
    pub no_open: bool,
}

struct CredentialLocation {
    path: PathBuf,
    manage_parent_permissions: bool,
}

#[derive(Serialize)]
struct CreatePairingRequest<'a> {
    code_challenge: &'a str,
    client: &'static str,
    client_version: &'static str,
    source: &'a str,
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
    source: String,
}

#[derive(Deserialize)]
struct ApiError {
    error: String,
}

#[derive(Serialize)]
struct CredentialFile<'a> {
    version: u8,
    endpoint: &'a str,
    source: &'a str,
    token_type: &'a str,
    access_token: &'a str,
    expires_at: u64,
    telegram_user_id: i64,
}

pub fn authorize_telegram(options: TelegramAuthOptions) -> Result<()> {
    let endpoint = normalize_endpoint(&options.endpoint)?;
    validate_source(&options.source)?;
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
            source: &options.source,
        })
        .send()
        .context("create Telegram pairing session")?
        .error_for_status()
        .context("infraBot rejected the pairing request")?
        .json::<CreatePairingResponse>()
        .context("decode pairing response")?;

    println!("authorize infraCLI source {} in Telegram:", options.source);
    println!("{}", pairing.verification_uri_complete);

    if !options.no_open && !open_uri(&pairing.verification_uri_complete) {
        println!("open the link manually if it did not open automatically");
    }

    println!("waiting for confirmation...");
    let token = poll_for_token(&client, &endpoint, &pairing, &verifier)?;
    if token.source != options.source {
        bail!("infraBot returned credentials for an unexpected source");
    }
    let path = write_credentials(&endpoint, &token)?;

    println!(
        "authorized source {} as Telegram user {}",
        token.source, token.telegram_user_id
    );
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
                if token.token_type != "Bearer"
                    || token.access_token.is_empty()
                    || token.source.is_empty()
                {
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
    let parsed = Url::parse(value.trim()).context("infraBot endpoint is not a valid URL")?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("infraBot endpoint must not contain credentials");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        bail!("infraBot endpoint must not contain a query or fragment");
    }

    let host = parsed.host_str().context("infraBot endpoint has no host")?;
    let secure = parsed.scheme() == "https";
    let local = parsed.scheme() == "http" && matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !secure && !local {
        bail!("infraBot endpoint must use HTTPS; HTTP is allowed only for localhost");
    }

    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

fn validate_source(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        bail!("source must contain 1-64 ASCII letters, digits, dots, dashes, or underscores");
    }
    Ok(())
}

fn random_token(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn credentials_location() -> Result<CredentialLocation> {
    if let Some(path) = env::var_os("INFRA_CREDENTIALS_FILE") {
        return Ok(CredentialLocation {
            path: PathBuf::from(path),
            manage_parent_permissions: false,
        });
    }
    if let Some(root) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(CredentialLocation {
            path: PathBuf::from(root).join("infra/credentials.json"),
            manage_parent_permissions: true,
        });
    }
    if let Some(home) = env::var_os("HOME") {
        return Ok(CredentialLocation {
            path: PathBuf::from(home).join(".config/infra/credentials.json"),
            manage_parent_permissions: true,
        });
    }
    bail!("cannot resolve credentials path; set INFRA_CREDENTIALS_FILE")
}

fn write_credentials(endpoint: &str, token: &TokenResponse) -> Result<PathBuf> {
    let location = credentials_location()?;
    let path = location.path;
    let parent = path.parent().context("credentials path has no parent")?;
    prepare_directory(parent, location.manage_parent_permissions)?;

    let document = CredentialFile {
        version: 1,
        endpoint,
        source: &token.source,
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
fn prepare_directory(path: &Path, manage_existing_permissions: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let existed = path.exists();
    fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    if manage_existing_permissions || !existed {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("secure {}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_directory(path: &Path, _manage_existing_permissions: bool) -> Result<()> {
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
        assert!(normalize_endpoint("http://localhost.evil.example").is_err());
        assert!(normalize_endpoint("https://user:secret@bot.example").is_err());
    }

    #[test]
    fn validates_source_identifiers() {
        assert!(validate_source("primary-vps").is_ok());
        assert!(validate_source("host.eu_1").is_ok());
        assert!(validate_source("").is_err());
        assert!(validate_source("host/name").is_err());
    }

    #[test]
    fn challenge_is_sha256_base64url() {
        let verifier = random_token(VERIFIER_BYTES);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
    }
}
