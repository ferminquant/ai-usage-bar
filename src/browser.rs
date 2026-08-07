//! Explicit, allowlisted browser hand-off destinations.
//!
//! The shell only opens these fixed provider pages after a user chooses the
//! corresponding context-menu item. It never accepts a URL from provider data,
//! copies browser cookies, injects headers, or scrapes the resulting page.

pub const OLLAMA_USAGE_URL: &str = "https://ollama.com/settings";
pub const KIMI_CONSOLE_URL: &str = "https://www.kimi.com/code/console";

/// Whether a browser hand-off URL is one of the fixed, user-invoked pages.
pub fn is_allowed_browser_url(url: &str) -> bool {
    matches!(url, OLLAMA_USAGE_URL | KIMI_CONSOLE_URL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_only_fixed_https_provider_pages() {
        assert!(is_allowed_browser_url(OLLAMA_USAGE_URL));
        assert!(is_allowed_browser_url(KIMI_CONSOLE_URL));
        assert!(!is_allowed_browser_url("http://ollama.com/settings"));
        assert!(!is_allowed_browser_url("https://ollama.com/settings?token=secret"));
        assert!(!is_allowed_browser_url("https://evil.example/"));
    }
}
