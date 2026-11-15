//! Continuation tokens.
//!
//! The engine paginates on the last key it returned, which is the simplest thing that
//! works. That key cannot go on the wire as-is: it comes back as a query parameter,
//! where `+` means space, so a key containing `+` would paginate to the wrong place.
//!
//! Hex keeps the token opaque and safe in a URL without reaching for another
//! dependency, and real S3 tokens are opaque too, so nothing is lost.

/// Turn the engine's cursor into a token for the wire.
pub fn encode(key: &str) -> String {
    hex::encode(key)
}

/// Turn a token from a client back into the engine's cursor.
///
/// `None` means the token was not produced by us, which is a client error rather than
/// something to paper over: resuming from a cursor we cannot read would silently return
/// the wrong page.
pub fn decode(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_round_trip_a_plain_key() {
        let token = encode("photos/2024/cat.jpg");

        assert_eq!(decode(&token).as_deref(), Some("photos/2024/cat.jpg"));
    }

    #[test]
    fn should_round_trip_a_key_that_would_break_in_a_query_string() {
        for key in ["a+b", "100%.txt", "a b", "caf\u{e9}", "a&b=c"] {
            let token = encode(key);

            assert_eq!(decode(&token).as_deref(), Some(key), "key {key:?}");
        }
    }

    #[test]
    fn should_produce_a_token_that_is_safe_in_a_url() {
        let token = encode("a+b c&d");

        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn should_reject_a_token_that_is_not_ours() {
        assert!(decode("not-hex").is_none());
        assert!(decode("ff0001").is_none(), "invalid utf-8 is not a key");
    }
}
