//! Slowing down whoever worketh through the space of feed addresses.
//!
//! Only *misses* are counted. A subscriber polling its own calendar every
//! quarter hour never hits this, however often it asks; someone trying
//! addresses that do not exist hits it within the minute.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Misses allowed before an address is turned away.
const ALLOWANCE: u32 = 20;
/// How long the count is remembered, and how long the turning-away lasteth.
const WINDOW: Duration = Duration::from_secs(600);
/// Past this many addresses, the stale ones are swept before more are added.
const SWEEP_AT: usize = 4096;

#[derive(Default)]
pub struct Guard {
    seen: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

impl Guard {
    /// Called when a lookup found nothing. True if this address has now had
    /// more than its share and should be refused.
    pub fn note_miss(&self, who: IpAddr) -> bool {
        let now = Instant::now();
        let Ok(mut seen) = self.seen.lock() else {
            // A poisoned lock must not take the feed route down with it.
            return false;
        };

        if seen.len() > SWEEP_AT {
            seen.retain(|_, (_, when)| now.duration_since(*when) < WINDOW);
        }

        let entry = seen.entry(who).or_insert((0, now));
        if now.duration_since(entry.1) >= WINDOW {
            *entry = (0, now);
        }
        entry.0 += 1;
        entry.0 > ALLOWANCE
    }

    /// Whether this address is already over its share, asked before the
    /// database is troubled at all.
    pub fn is_barred(&self, who: IpAddr) -> bool {
        let Ok(seen) = self.seen.lock() else { return false };
        seen.get(&who).is_some_and(|(count, when)| *count > ALLOWANCE && when.elapsed() < WINDOW)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn somebody() -> IpAddr {
        "203.0.113.7".parse().unwrap()
    }

    #[test]
    fn a_handful_of_misses_is_nothing() {
        let guard = Guard::default();
        for _ in 0..ALLOWANCE {
            assert!(!guard.note_miss(somebody()), "an honest typo must not bar anyone");
        }
        assert!(!guard.is_barred(somebody()));
    }

    #[test]
    fn working_through_the_space_is_stopped() {
        let guard = Guard::default();
        for _ in 0..=ALLOWANCE {
            guard.note_miss(somebody());
        }
        assert!(guard.is_barred(somebody()));
    }

    #[test]
    fn one_address_being_barred_leaves_the_rest_alone() {
        let guard = Guard::default();
        for _ in 0..=ALLOWANCE {
            guard.note_miss(somebody());
        }
        let innocent: IpAddr = "198.51.100.4".parse().unwrap();
        assert!(!guard.is_barred(innocent), "the whole world must not share one bucket");
    }
}
