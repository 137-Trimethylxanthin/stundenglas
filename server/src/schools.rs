//! Small helpers for what a user typeth when linking a school.

/// People paste whole URLs. Take the host out of whatever they gave.
pub fn tidy_server(raw: &str) -> String {
    let raw = raw.trim();
    let raw = raw.strip_prefix("https://").or_else(|| raw.strip_prefix("http://")).unwrap_or(raw);
    let host = raw.split('/').next().unwrap_or("").trim().trim_end_matches('.');
    if host.contains(' ') || !host.contains('.') {
        String::new()
    } else {
        host.to_ascii_lowercase()
    }
}

#[cfg(test)]
mod tests {
    use super::tidy_server;

    #[test]
    fn a_bare_host_passes_through() {
        assert_eq!(tidy_server("example.webuntis.com"), "example.webuntis.com");
        assert_eq!(tidy_server("  Example.WebUntis.com  "), "example.webuntis.com");
    }

    #[test]
    fn a_pasted_url_is_reduced_to_its_host() {
        assert_eq!(tidy_server("https://ex.webuntis.com/WebUntis/?school=x"), "ex.webuntis.com");
        assert_eq!(tidy_server("http://ex.webuntis.com/"), "ex.webuntis.com");
        assert_eq!(tidy_server("ex.webuntis.com/WebUntis"), "ex.webuntis.com");
    }

    #[test]
    fn nonsense_yields_nothing_rather_than_a_bad_request_later() {
        assert_eq!(tidy_server(""), "");
        assert_eq!(tidy_server("not a host"), "");
        assert_eq!(tidy_server("localhost"), "", "a host without a dot is not a school");
    }
}
