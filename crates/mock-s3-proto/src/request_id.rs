//! Request ids.
//!
//! Every S3 response carries one, and support conversations about real S3 start with it.
//! A counter is enough here: nothing depends on it being unguessable or unique across
//! restarts, only on it tying a log line to a response.

use std::sync::atomic::{AtomicU64, Ordering};

pub const HEADER: &str = "x-amz-request-id";

pub fn next() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);

    format!("{:016X}", NEXT.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_hand_out_a_different_id_each_time() {
        assert_ne!(next(), next());
    }

    #[test]
    fn should_be_fixed_width_hex() {
        let id = next();

        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
