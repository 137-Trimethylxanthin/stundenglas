//! The red block a refused link carrieth in its calendar.
//!
//! Not mail, and not said once: worked out afresh on every look, dated today
//! where the school is, for the feed and for Google alike.

use uuid::Uuid;

/// The red notice a refused password puteth in the calendar itself, on
/// today's date where the school is. Mail is sent once and may be missed; the
/// calendar is looked at every morning.
///
/// The link leadeth to the very field the new password goeth in.
pub fn notice(
    public_url: &str,
    account: Uuid,
    zone: chrono_tz::Tz,
    now: chrono::DateTime<chrono::Utc>,
) -> stundenglas_core::Relogin {
    stundenglas_core::Relogin {
        key: account.simple().to_string(),
        day: now.with_timezone(&zone).date_naive(),
        zone,
        // The query surviveth signing in, which the fragment alone doth not.
        link: format!("{public_url}/?relogin={account}#pw-{account}"),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    #[test]
    fn the_notice_is_dated_where_the_school_is() {
        let account = uuid::Uuid::from_u128(0x0b8f_5c2e_9a1d_4e7f_8a6b_3c2d_1e0f_9a8b);
        // 23:30 UTC on the 15th is already the 16th in Vienna.
        let late = chrono::Utc.with_ymd_and_hms(2026, 10, 15, 23, 30, 0).unwrap();
        let notice =
            super::notice("https://stunden.example.test", account, chrono_tz::Europe::Vienna, late);
        assert_eq!(notice.day, chrono::NaiveDate::from_ymd_opt(2026, 10, 16).unwrap());
        assert_eq!(
            notice.link,
            "https://stunden.example.test/?relogin=0b8f5c2e-9a1d-4e7f-8a6b-3c2d1e0f9a8b\
             #pw-0b8f5c2e-9a1d-4e7f-8a6b-3c2d1e0f9a8b",
            "the dashboard's password field hath this id"
        );
        assert!(notice.key.chars().all(|c| c.is_ascii_digit() || ('a'..='v').contains(&c)));
    }
}
