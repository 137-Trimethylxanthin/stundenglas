//! Google's installed-app dance, and the refreshing thereof.
//!
//! The token store keepeth the shape Google's own libraries use, so that
//! `token.json` may be read and written by other tools besides.

use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const SCOPE: &str = "https://www.googleapis.com/auth/calendar";

#[derive(Deserialize)]
struct Installed {
    client_id: String,
    client_secret: String,
    #[serde(default = "auth_endpoint")]
    auth_uri: String,
    #[serde(default = "token_endpoint")]
    token_uri: String,
}

#[derive(Deserialize)]
struct ClientSecrets {
    installed: Option<Installed>,
    web: Option<Installed>,
}

fn auth_endpoint() -> String {
    "https://accounts.google.com/o/oauth2/auth".to_owned()
}

fn token_endpoint() -> String {
    "https://oauth2.googleapis.com/token".to_owned()
}

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    #[serde(default)]
    token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default = "token_endpoint")]
    token_uri: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    client_secret: String,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    expiry: Option<String>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct Granted {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

pub struct Authoriser {
    http: reqwest::Client,
    credentials: PathBuf,
    token_file: PathBuf,
}

impl Authoriser {
    pub fn new(http: reqwest::Client, credentials: PathBuf, token_file: PathBuf) -> Self {
        Self { http, credentials, token_file }
    }

    /// An access token, be it refreshed, reused, or newly begged of the user.
    pub async fn access_token(&self) -> Result<String> {
        if let Some(stored) = self.read_store()? {
            if let Some(live) = still_good(&stored) {
                return Ok(live);
            }
            if !stored.refresh_token.is_empty() {
                return self.refresh(stored).await;
            }
        }
        self.consent().await
    }

    fn read_store(&self) -> Result<Option<Stored>> {
        if !self.token_file.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&self.token_file)
            .with_context(|| format!("reading {}", self.token_file.display()))?;
        Ok(serde_json::from_str(&raw).ok())
    }

    fn client(&self) -> Result<Installed> {
        if !self.credentials.exists() {
            bail!(
                "{} not found — make an OAuth client of type 'Desktop app' in the \
                 Google Cloud console and save its JSON there",
                self.credentials.display()
            );
        }
        let raw = fs::read_to_string(&self.credentials)?;
        let secrets: ClientSecrets =
            serde_json::from_str(&raw).context("this does not look like a client-secret file")?;
        secrets
            .installed
            .or(secrets.web)
            .ok_or_else(|| anyhow!("the client-secret file hath neither 'installed' nor 'web'"))
    }

    async fn refresh(&self, mut stored: Stored) -> Result<String> {
        let client = self.client()?;
        let client_id = if stored.client_id.is_empty() {
            client.client_id.clone()
        } else {
            stored.client_id.clone()
        };
        let client_secret = if stored.client_secret.is_empty() {
            client.client_secret.clone()
        } else {
            stored.client_secret.clone()
        };

        let reply = self
            .http
            .post(&stored.token_uri)
            .form(&[
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("refresh_token", stored.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .context("refreshing the Google token")?;

        let status = reply.status();
        let body = reply.text().await?;
        if !status.is_success() {
            if body.contains("invalid_grant") {
                bail!(
                    "Google refused the refresh token ({status}). Whilst the OAuth consent \
                     screen standeth in 'Testing', such tokens die after seven days — publish \
                     the app, then delete {} and run again to consent anew.",
                    self.token_file.display()
                );
            }
            bail!("{status} refreshing the token: {}", body.chars().take(200).collect::<String>());
        }

        let granted: Granted =
            serde_json::from_str(&body).context("decoding the refreshed token")?;
        stored.token = granted.access_token.clone();
        stored.client_id = client_id;
        stored.client_secret = client_secret;
        if let Some(fresh) = granted.refresh_token {
            stored.refresh_token = fresh;
        }
        stored.expiry = Some(expiry_of(granted.expires_in));
        self.write_store(&stored)?;
        Ok(granted.access_token)
    }

    /// Bindeth a loopback port, sendeth the user thither, and taketh the code.
    async fn consent(&self) -> Result<String> {
        // Under a timer there is no one to read a URL nor click it; better to
        // fail loudly than to block a oneshot unit for ever.
        if !std::io::stdin().is_terminal() {
            bail!(
                "no usable {} and no terminal to consent in — run this once by hand to \
                 authorise, then let the timer take over",
                self.token_file.display()
            );
        }
        let client = self.client()?;
        let listener = TcpListener::bind("127.0.0.1:0").await.context("binding a loopback port")?;
        let port = listener.local_addr()?.port();
        let redirect = format!("http://localhost:{port}");

        let verifier = URL_SAFE_NO_PAD.encode(random_bytes(64)?);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = URL_SAFE_NO_PAD.encode(random_bytes(16)?);

        let authorise = url::Url::parse_with_params(
            &client.auth_uri,
            &[
                ("response_type", "code"),
                ("client_id", &client.client_id),
                ("redirect_uri", &redirect),
                ("scope", SCOPE),
                ("state", &state),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
                ("access_type", "offline"),
                ("prompt", "consent"),
            ],
        )?;

        println!("\nGrant this program sight of thy calendar; open:\n\n  {authorise}\n");
        println!("Waiting on {redirect} …");

        let code = await_code(&listener, &state).await?;

        let reply = self
            .http
            .post(&client.token_uri)
            .form(&[
                ("code", code.as_str()),
                ("client_id", client.client_id.as_str()),
                ("client_secret", client.client_secret.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("grant_type", "authorization_code"),
                ("code_verifier", verifier.as_str()),
            ])
            .send()
            .await
            .context("exchanging the authorisation code")?;

        let status = reply.status();
        let body = reply.text().await?;
        if !status.is_success() {
            bail!("{status} exchanging the code: {}", body.chars().take(300).collect::<String>());
        }
        let granted: Granted = serde_json::from_str(&body).context("decoding the granted token")?;

        let stored = Stored {
            token: granted.access_token.clone(),
            refresh_token: granted.refresh_token.unwrap_or_default(),
            token_uri: client.token_uri,
            client_id: client.client_id,
            client_secret: client.client_secret,
            scopes: vec![SCOPE.to_owned()],
            expiry: Some(expiry_of(granted.expires_in)),
            rest: serde_json::Map::new(),
        };
        self.write_store(&stored)?;
        Ok(granted.access_token)
    }

    fn write_store(&self, stored: &Stored) -> Result<()> {
        fs::write(&self.token_file, serde_json::to_vec_pretty(stored)?)
            .with_context(|| format!("writing {}", self.token_file.display()))?;
        harden(&self.token_file)
    }
}

fn still_good(stored: &Stored) -> Option<String> {
    if stored.token.is_empty() {
        return None;
    }
    let expiry = stored.expiry.as_deref()?;
    let when = DateTime::parse_from_rfc3339(expiry)
        .map(|t| t.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(expiry, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|n| n.and_utc())
        })
        .ok()?;
    // Spend it only whilst a minute of life remaineth.
    (when > Utc::now() + Duration::seconds(60)).then(|| stored.token.clone())
}

fn expiry_of(expires_in: Option<i64>) -> String {
    let seconds = expires_in.unwrap_or(3600);
    (Utc::now() + Duration::seconds(seconds)).format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

async fn await_code(listener: &TcpListener, state: &str) -> Result<String> {
    loop {
        let (stream, _) = listener.accept().await.context("accepting the redirect")?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            continue;
        }

        let target = line.split_whitespace().nth(1).unwrap_or("/");
        let parsed = url::Url::parse(&format!("http://localhost{target}"))?;
        let pairs: Vec<(String, String)> =
            parsed.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let find = |want: &str| pairs.iter().find(|(k, _)| k == want).map(|(_, v)| v.clone());

        let (body, outcome) = match (find("code"), find("error")) {
            (_, Some(err)) => {
                (format!("Refused: {err}"), Err(anyhow!("Google refused consent: {err}")))
            }
            (Some(code), _) if find("state").as_deref() == Some(state) => {
                ("Granted. Thou mayest close this page.".to_owned(), Ok(code))
            }
            (Some(_), _) => {
                ("State mismatch.".to_owned(), Err(anyhow!("state mismatch; flow abandoned")))
            }
            _ => {
                // Browsers beg for /favicon.ico; pay them no mind.
                let mut sink = reader.into_inner();
                let _ =
                    sink.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await;
                continue;
            }
        };

        let page = format!(
            "<!doctype html><meta charset=utf-8><title>stundenglas</title>\
             <body style=\"font-family:system-ui;padding:3rem\"><h1>{body}</h1>"
        );
        let mut sink = reader.into_inner();
        let _ = sink
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                )
                .as_bytes(),
            )
            .await;
        let _ = sink.shutdown().await;
        return outcome;
    }
}

/// `/dev/urandom` never endeth, so we take an exact measure and no more.
fn random_bytes(n: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = vec![0_u8; n];
    fs::File::open("/dev/urandom")
        .context("opening /dev/urandom")?
        .read_exact(&mut bytes)
        .context("drawing randomness")?;
    Ok(bytes)
}

#[cfg(unix)]
fn harden(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", path.display()))
}

#[cfg(not(unix))]
fn harden(_path: &Path) -> Result<()> {
    Ok(())
}
