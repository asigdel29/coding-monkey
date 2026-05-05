/*
   File: crates/agents/src/redact.rs

   Purpose
   Scrub secrets from agent stdout before they hit the audit log or any
   other on-disk store. Rules are conservative — we'd rather over-redact
   than leak. Add new patterns here when a new vendor's key shape comes
   into use.

   The redactor is byte-stream-friendly: callers pass strings as they
   stream from the PTY, and the redactor returns the same shape back
   with sensitive substrings replaced by `[REDACTED:<kind>]`.

   History
   Date         Author          Changes
   2026-05-05   Anubhav Sigdel  initial Rust port from packages/agents/src/redact.ts
*/

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

#[derive(Debug)]
struct Pattern {
    regex: Regex,
    label: &'static str,
}

static PATTERNS: Lazy<Vec<Pattern>> = Lazy::new(|| {
    fn p(re: &str, label: &'static str) -> Pattern {
        Pattern {
            regex: Regex::new(re).expect("valid regex"),
            label,
        }
    }
    vec![
        p(r"sk-ant-[A-Za-z0-9_\-]{20,}", "anthropic-key"),
        p(r"sk-(proj-)?[A-Za-z0-9]{20,}", "openai-key"),
        p(r"gh[pousr]_[A-Za-z0-9]{30,}", "github-token"),
        p(r"AKIA[0-9A-Z]{16}", "aws-access-key"),
        p(r"xox[baprs]-[A-Za-z0-9-]{10,}", "slack-token"),
        p(r"AIza[0-9A-Za-z\-_]{35}", "google-api-key"),
        // PEM blocks — match the header so we replace the whole block.
        p(
            r"(?s)-----BEGIN (?:RSA |EC |DSA |OPENSSH |)PRIVATE KEY-----.*?-----END (?:RSA |EC |DSA |OPENSSH |)PRIVATE KEY-----",
            "private-key",
        ),
    ]
});

/// Replace every recognized secret in `s` with `[REDACTED:<kind>]`.
///
/// Allocates only when a match is found. Safe to call on every chunk
/// streaming from a PTY — empty strings short-circuit.
pub fn redact(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let mut out = std::borrow::Cow::Borrowed(s);
    for p in PATTERNS.iter() {
        if p.regex.is_match(&out) {
            let replacement = format!("[REDACTED:{}]", p.label);
            out = std::borrow::Cow::Owned(p.regex.replace_all(&out, replacement).into_owned());
        }
    }
    out.into_owned()
}

/// Recursively redact every string node in a JSON value. Useful for
/// scrubbing structured tool-result blobs before audit-logging them.
pub fn redact_object(v: &mut Value) {
    match v {
        Value::String(s) => *s = redact(s),
        Value::Array(arr) => arr.iter_mut().for_each(redact_object),
        Value::Object(map) => map.values_mut().for_each(redact_object),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_anthropic_key() {
        let input = "ANTHROPIC_API_KEY=sk-ant-api03-1234567890abcdefghij1234";
        let out = redact(input);
        assert!(out.contains("[REDACTED:anthropic-key]"));
        assert!(!out.contains("sk-ant-api03"));
    }

    #[test]
    fn redacts_openai_key() {
        let out = redact("token=sk-proj-1234567890ABCDEFGHIJKLMNOP");
        assert!(out.contains("[REDACTED:openai-key]"));
    }

    #[test]
    fn redacts_github_token() {
        let out = redact("ghp_1234567890123456789012345678901234567890");
        assert!(out.contains("[REDACTED:github-token]"));
    }

    #[test]
    fn empty_string_is_noop() {
        assert!(redact("").is_empty());
    }

    #[test]
    fn untouched_when_clean() {
        let input = "no secrets here, just plain text";
        assert_eq!(redact(input), input);
    }

    #[test]
    fn redact_object_walks_nested() {
        let mut v: Value = serde_json::json!({
            "outer": "ghp_1234567890123456789012345678901234567890",
            "nested": { "inner": ["sk-ant-api03-1234567890abcdefghij1234"] },
        });
        redact_object(&mut v);
        let s = v.to_string();
        assert!(s.contains("[REDACTED:github-token]"));
        assert!(s.contains("[REDACTED:anthropic-key]"));
    }
}
