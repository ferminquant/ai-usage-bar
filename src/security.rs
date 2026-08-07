//! Small, dependency-free security helpers shared by diagnostics and the UI.
//!
//! Provider adapters should prefer typed error codes and never attach upstream
//! bodies. These helpers are a second boundary for test doubles, future
//! adapters, and diagnostic renderers that may receive human-readable text.

const REDACTED: &str = "[REDACTED]";

/// Redact credential-shaped values from text before it reaches a diagnostic,
/// tooltip, clipboard payload, or log.
///
/// The function is intentionally conservative: ordinary prose such as
/// "provider token must not escape" is retained, while named credentials,
/// authorization/cookie headers, sensitive query parameters, JWT-shaped
/// values, and well-known provider-key prefixes are replaced.
pub fn redact_sensitive_text(input: &str) -> String {
    let mut ranges = marked_value_ranges(input);
    ranges.extend(query_value_ranges(input));
    ranges.extend(bare_token_ranges(input));
    apply_ranges(input, ranges)
}

/// Return an identifier suitable for a user-visible diagnostic.
///
/// Provider adapters normally provide a stable hashed identifier already. If
/// a future adapter accidentally supplies an email, path, or other unsafe
/// identifier, the display/export boundary keeps the value stable without
/// exposing the original string.
pub fn safe_identifier(value: &str) -> String {
    let trimmed = value.trim();
    let safe = !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
        });
    if safe {
        trimmed.to_string()
    } else {
        format!("account-{:016x}", stable_hash(trimmed.as_bytes()))
    }
}

fn marked_value_ranges(input: &str) -> Vec<(usize, usize)> {
    const MARKERS: &[&str] = &[
        "proxy-authorization",
        "authorization",
        "set-cookie",
        "cookie",
        "access_token",
        "refresh_token",
        "id_token",
        "client_secret",
        "api_key",
        "apikey",
        "password",
        "secret",
        "token",
    ];

    let lower = input.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let mut ranges = Vec::new();
    for marker in MARKERS {
        let mut search_from = 0;
        while search_from < lower.len() {
            let Some(relative) = lower[search_from..].find(marker) else {
                break;
            };
            let start = search_from + relative;
            let marker_end = start + marker.len();
            search_from = marker_end;

            if !has_identifier_boundaries(bytes, start, marker_end) {
                continue;
            }

            let mut cursor = marker_end;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            let separator = bytes.get(cursor).copied();
            if !matches!(separator, Some(b':') | Some(b'=')) {
                continue;
            }
            let is_header = matches!(
                *marker,
                "proxy-authorization" | "authorization" | "set-cookie" | "cookie"
            );
            cursor += 1;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if cursor >= bytes.len() {
                continue;
            }

            let (value_start, value_end) = if is_header {
                (cursor, line_end(input, cursor))
            } else {
                (cursor, value_end(input, cursor))
            };
            if value_start < value_end {
                ranges.push((value_start, value_end));
            }
        }
    }
    ranges
}

fn query_value_ranges(input: &str) -> Vec<(usize, usize)> {
    let bytes = input.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !matches!(bytes[index], b'?' | b'&') {
            index += 1;
            continue;
        }
        let key_start = index + 1;
        let mut equals = key_start;
        while equals < bytes.len() && !matches!(bytes[equals], b'=' | b'&' | b'#' | b' ') {
            equals += 1;
        }
        if equals >= bytes.len() || bytes[equals] != b'=' {
            index = equals.saturating_add(1);
            continue;
        }
        let key = input[key_start..equals].to_ascii_lowercase();
        if is_sensitive_query_key(&key) {
            let value_start = equals + 1;
            let value_end = value_end(input, value_start);
            if value_start < value_end {
                ranges.push((value_start, value_end));
            }
        }
        index = equals + 1;
    }
    ranges
}

fn is_sensitive_query_key(key: &str) -> bool {
    matches!(
        key,
        "access_token"
            | "refresh_token"
            | "id_token"
            | "token"
            | "api_key"
            | "apikey"
            | "client_secret"
            | "secret"
            | "code"
            | "state"
            | "session"
            | "cookie"
    )
}

fn bare_token_ranges(input: &str) -> Vec<(usize, usize)> {
    const PREFIXES: &[&str] = &["ghp_", "github_pat_", "sk-proj-", "sk-", "oc_sk_"];
    let mut ranges = Vec::new();
    for prefix in PREFIXES {
        let mut search_from = 0;
        while let Some(relative) = input[search_from..].find(prefix) {
            let start = search_from + relative;
            let end = value_end(input, start);
            if end > start + prefix.len() {
                ranges.push((start, end));
            }
            search_from = start + prefix.len();
            if search_from >= input.len() {
                break;
            }
        }
    }

    let mut search_from = 0;
    while let Some(relative) = input[search_from..].find("eyJ") {
        let start = search_from + relative;
        let end = value_end(input, start);
        let token = &input[start..end];
        if token.matches('.').count() >= 2 && token.len() >= 9 {
            ranges.push((start, end));
        }
        search_from = start + 3;
        if search_from >= input.len() {
            break;
        }
    }
    ranges
}

fn has_identifier_boundaries(bytes: &[u8], start: usize, end: usize) -> bool {
    let before_is_identifier = start > 0
        && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
    let after_is_identifier = end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_');
    !before_is_identifier && !after_is_identifier
}

fn line_end(input: &str, start: usize) -> usize {
    input[start..]
        .find(['\r', '\n'])
        .map_or(input.len(), |offset| start + offset)
}

fn value_end(input: &str, start: usize) -> usize {
    let bytes = input.as_bytes();
    if start >= bytes.len() {
        return start;
    }
    if matches!(bytes[start], b'"' | b'\'') {
        let quote = bytes[start];
        return input[start + 1..]
            .find(quote as char)
            .map_or(input.len(), |offset| start + 2 + offset);
    }
    let mut end = start;
    while end < bytes.len()
        && !matches!(bytes[end], b'\r' | b'\n' | b' ' | b'\t' | b',' | b';' | b'&' | b'#' | b'}' | b']' | b')')
    {
        end += 1;
    }
    end
}

fn apply_ranges(input: &str, mut ranges: Vec<(usize, usize)>) -> String {
    if ranges.is_empty() {
        return input.to_string();
    }
    ranges.sort_unstable_by_key(|(start, _)| *start);
    let mut merged = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        if start >= end {
            continue;
        }
        if let Some((_, previous_end)) = merged.last_mut() {
            if start <= *previous_end {
                *previous_end = (*previous_end).max(end);
                continue;
            }
        }
        merged.push((start, end));
    }

    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    for (start, end) in merged {
        output.push_str(&input[cursor..start]);
        output.push_str(REDACTED);
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_authorization_and_cookie_headers() {
        let input = "Authorization: Bearer short-token\nCookie: session=short-cookie; theme=dark";
        let output = redact_sensitive_text(input);
        assert!(!output.contains("short-token"));
        assert!(!output.contains("short-cookie"));
        assert!(output.contains("Authorization: [REDACTED]"));
        assert!(output.contains("Cookie: [REDACTED]"));
    }

    #[test]
    fn redacts_named_tokens_and_sensitive_query_values() {
        let input = "refresh_token=short-refresh api_key=\"quoted-key\" https://example.test/callback?code=short-code&format=json";
        let output = redact_sensitive_text(input);
        assert!(!output.contains("short-refresh"));
        assert!(!output.contains("quoted-key"));
        assert!(!output.contains("short-code"));
        assert!(output.contains("format=json"));
        assert!(output.contains("api_key=[REDACTED]"));
    }

    #[test]
    fn redacts_jwt_and_known_key_prefixes() {
        let input = "ghp_demo oc_sk_demo eyJab.c.d";
        let output = redact_sensitive_text(input);
        assert_eq!(output, "[REDACTED] [REDACTED] [REDACTED]");
    }

    #[test]
    fn keeps_ordinary_diagnostic_prose() {
        assert_eq!(
            redact_sensitive_text("provider token must not escape"),
            "provider token must not escape"
        );
    }

    #[test]
    fn safe_identifier_hashes_emails_and_paths_but_keeps_generated_ids() {
        assert_eq!(safe_identifier("codex-deadbeef"), "codex-deadbeef");
        let email = safe_identifier("person@example.com");
        let path = safe_identifier("C:/Users/person/.config/auth.json");
        assert!(email.starts_with("account-"));
        assert!(path.starts_with("account-"));
        assert_ne!(email, path);
    }
}
