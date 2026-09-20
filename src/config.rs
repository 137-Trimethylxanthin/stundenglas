use anyhow::{Context, Result, bail};
use std::{fs, path::Path};

/// Credentials whereby we maketh ourselves known unto WebUntis.
#[derive(Clone)]
pub struct Settings {
    pub server: String,
    pub school: String,
    pub user: String,
    pub password: String,
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("server", &self.server)
            .field("school", &self.school)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        let raw =
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;

        let mut server = String::new();
        let mut school = String::new();
        let mut user = String::new();
        let mut password = String::new();

        for line in raw.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches(['"', '\'']).to_owned();
            match key.trim() {
                "UNTIS_SERVER" => server = value,
                "UNTIS_SCHOOL" => school = value,
                "UNTIS_USER" => user = value,
                "UNTIS_PASS" => password = value,
                _ => {}
            }
        }

        for (name, value) in [
            ("UNTIS_SERVER", &server),
            ("UNTIS_SCHOOL", &school),
            ("UNTIS_USER", &user),
            ("UNTIS_PASS", &password),
        ] {
            if value.is_empty() {
                bail!("{name} is missing from {}", path.display());
            }
        }
        Ok(Self { server, school, user, password })
    }

    pub fn base_url(&self) -> String {
        format!("https://{}", self.server)
    }
}
