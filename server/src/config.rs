//! Everything the server needeth to know, taken from the environment so that
//! no secret need ever sit in the repository.

use anyhow::{Context, Result, bail};
use std::time::Duration;

use crate::crypto::Sealer;

/// A Google OAuth *web* client. Absent, and the push option simply is not
/// offered; the calendar links work without it.
#[derive(Clone)]
pub struct GoogleClient {
    pub id: String,
    pub secret: String,
}

#[derive(Clone)]
pub struct Config {
    /// Where Postgres liveth.
    pub database_url: String,
    /// What we bind to.
    pub listen: String,
    /// How the world reacheth us, used to build feed URLs.
    pub public_url: String,
    /// Supabase's auth endpoint, spoken to by us alone.
    pub supabase_url: String,
    pub supabase_anon_key: String,
    pub supabase_service_key: String,
    /// Sealeth and unsealeth the stored passwords.
    pub sealer: Sealer,
    /// How often each account is refreshed from WebUntis.
    pub sync_every: Duration,
    /// How many accounts are refreshed at once.
    pub sync_lanes: usize,
    pub google: Option<GoogleClient>,
    /// Addresses admitted at once, and allowed to admit others.
    pub admin_emails: Vec<String>,
    /// How long a timetable nobody refresheth is kept before it is forgotten.
    pub keep_days: i64,
}

fn need(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("{key} is not set"))
}

fn or(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_owned())
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let key = need("STUNDENGLAS_KEY").context(
            "no encryption key. Generate one with `stundenglas-server --generate-key` \
             and keep it somewhere the database is not",
        )?;
        let sealer = Sealer::from_base64(&key)?;

        let minutes: u64 = or("SYNC_EVERY_MINUTES", "30").parse().unwrap_or(30);
        if minutes < 5 {
            bail!("SYNC_EVERY_MINUTES below five would hammer the school's server");
        }
        let lanes: usize = or("SYNC_LANES", "4").parse().unwrap_or(4);

        Ok(Self {
            database_url: need("DATABASE_URL")?,
            listen: or("LISTEN", "127.0.0.1:8080"),
            public_url: or("PUBLIC_URL", "http://127.0.0.1:8080").trim_end_matches('/').to_owned(),
            supabase_url: need("SUPABASE_URL")?.trim_end_matches('/').to_owned(),
            supabase_anon_key: need("SUPABASE_ANON_KEY")?,
            supabase_service_key: need("SUPABASE_SERVICE_KEY")?,
            sealer,
            sync_every: Duration::from_secs(minutes * 60),
            sync_lanes: lanes.clamp(1, 32),
            google: match (std::env::var("GOOGLE_CLIENT_ID"), std::env::var("GOOGLE_CLIENT_SECRET"))
            {
                (Ok(id), Ok(secret)) if !id.is_empty() && !secret.is_empty() => {
                    Some(GoogleClient { id, secret })
                }
                _ => None,
            },
            keep_days: or("KEEP_DAYS", "180").parse().unwrap_or(180).clamp(7, 3650),
            admin_emails: or("ADMIN_EMAILS", "")
                .split(',')
                .map(|a| a.trim().to_ascii_lowercase())
                .filter(|a| !a.is_empty())
                .collect(),
        })
    }

    pub fn feed_url(&self, token: &str) -> String {
        format!("{}/cal/{token}.ics", self.public_url)
    }

    /// The same address under the scheme calendars answer to. Tapping it on a
    /// phone opens the subscribe dialogue instead of downloading a file once,
    /// which is the difference between a calendar that follows the timetable
    /// and a snapshot of the day it was fetched.
    pub fn webcal_url(&self, token: &str) -> String {
        let bare = self
            .public_url
            .strip_prefix("https://")
            .or_else(|| self.public_url.strip_prefix("http://"))
            .unwrap_or(&self.public_url);
        format!("webcal://{bare}/cal/{token}.ics")
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("listen", &self.listen)
            .field("public_url", &self.public_url)
            .field("supabase_url", &self.supabase_url)
            .field("database_url", &"<withheld>")
            .field("supabase_anon_key", &"<withheld>")
            .field("supabase_service_key", &"<withheld>")
            .field("sealer", &self.sealer)
            .field("sync_every", &self.sync_every)
            .field("sync_lanes", &self.sync_lanes)
            .field("google", &self.google.as_ref().map(|_| "<configured>"))
            .field("keep_days", &self.keep_days)
            .field("admin_emails", &self.admin_emails)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_secret_is_printed() {
        let config = Config {
            database_url: "postgres://user:hunter2@host/db".to_owned(),
            listen: "127.0.0.1:8080".to_owned(),
            public_url: "https://example.test".to_owned(),
            supabase_url: "http://127.0.0.1:54321".to_owned(),
            supabase_anon_key: "anon-key-material".to_owned(),
            supabase_service_key: "service-key-material".to_owned(),
            sealer: Sealer::from_bytes(&[7; 32]).unwrap(),
            sync_every: Duration::from_secs(1800),
            sync_lanes: 4,
            google: Some(GoogleClient {
                id: "client-id-material".to_owned(),
                secret: "client-secret-material".to_owned(),
            }),
            admin_emails: vec![],
            keep_days: 180,
        };
        let shown = format!("{config:?}");
        for secret in
            ["hunter2", "anon-key-material", "service-key-material", "client-secret-material"]
        {
            assert!(!shown.contains(secret), "{secret} leaked into {shown}");
        }
    }

    #[test]
    fn feed_urls_have_no_double_slash() {
        let mut config = Config {
            database_url: String::new(),
            listen: String::new(),
            public_url: "https://example.test".to_owned(),
            supabase_url: String::new(),
            supabase_anon_key: String::new(),
            supabase_service_key: String::new(),
            sealer: Sealer::from_bytes(&[0; 32]).unwrap(),
            sync_every: Duration::from_secs(1800),
            sync_lanes: 1,
            google: None,
            admin_emails: vec![],
            keep_days: 180,
        };
        assert_eq!(config.feed_url("abc"), "https://example.test/cal/abc.ics");
        assert_eq!(config.webcal_url("abc"), "webcal://example.test/cal/abc.ics");
        config.public_url = "https://example.test".to_owned();
        assert!(!config.feed_url("abc").contains("//cal"));
    }
}
