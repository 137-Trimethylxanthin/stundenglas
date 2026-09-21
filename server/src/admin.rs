//! What an operator needeth before there is a web page to click.

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use uuid::Uuid;

use crate::{AppState, db, db::NewAccount};

/// A token for a feed URL: 32 bytes of randomness, url-safe.
pub fn mint_token() -> Result<String> {
    use std::io::Read;
    let mut bytes = [0_u8; 32];
    std::fs::File::open("/dev/urandom")
        .context("opening /dev/urandom")?
        .read_exact(&mut bytes)
        .context("drawing randomness")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Attach a WebUntis login to a user and hand back a calendar URL.
pub async fn add_account(state: &AppState, new: &NewAccount) -> Result<String> {
    if new.password.is_empty() {
        bail!("a password is wanted");
    }
    let account = db::add_account(&state.pool, &state.config.sealer, new).await?;

    // One feed is made at once, so there is something to subscribe to.
    let existing = db::feeds_of(&state.pool, account.id).await?;
    let token = match existing.first() {
        Some(feed) => feed.token.clone(),
        None => {
            let token = mint_token()?;
            db::create_feed(&state.pool, account.id, &token).await?;
            token
        }
    };
    Ok(state.config.feed_url(&token))
}

pub async fn list(state: &AppState, user_id: Uuid) -> Result<()> {
    for account in db::accounts_of(&state.pool, user_id).await? {
        let who = account.display_name.clone().unwrap_or_else(|| "?".into());
        println!(
            "{}  {}@{}  {}  {}  {}",
            account.id,
            account.username,
            account.school,
            who,
            account.timezone.name(),
            if account.enabled { "enabled" } else { "disabled" }
        );
        for feed in db::feeds_of(&state.pool, account.id).await? {
            println!("    {}", state.config.feed_url(&feed.token));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::mint_token;

    #[test]
    fn tokens_are_long_url_safe_and_unlike_one_another() {
        let a = mint_token().unwrap();
        let b = mint_token().unwrap();
        assert_ne!(a, b);
        assert!(a.len() >= 40, "32 bytes of entropy should not shrink below 40 chars");
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        // and the feed route must be willing to look it up
        assert!(crate::feed::token_is_plausible(&a));
    }
}
