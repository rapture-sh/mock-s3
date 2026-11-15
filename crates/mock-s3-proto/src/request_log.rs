//! A ring buffer of recent requests, for the admin API and the console.
//!
//! Logs answer "what happened"; this answers "what did my SDK actually send", which is
//! the question a mock gets asked. An SDK makes calls the caller never wrote — a
//! `GetBucketLocation` before a listing, a switch to multipart above a threshold — and
//! being able to read them back is the difference between debugging your code and
//! guessing at the client library.
//!
//! Bounded on purpose: a long test run must not grow this without limit.

use std::collections::VecDeque;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RequestEntry {
    pub at: DateTime<Utc>,
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    pub status: u16,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

pub struct RequestLog {
    capacity: usize,
    entries: Mutex<VecDeque<RequestEntry>>,
}

impl RequestLog {
    /// A log holding at most `capacity` entries. Zero disables it.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.capacity > 0
    }

    pub fn record(&self, entry: RequestEntry) {
        if !self.is_enabled() {
            return;
        }

        // A poisoned lock would mean a panic while recording; dropping the entry is
        // better than propagating that into the request path.
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        while entries.len() >= self.capacity {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// The most recent entries, newest first.
    pub fn recent(&self, limit: usize) -> Vec<RequestEntry> {
        let Ok(entries) = self.entries.lock() else {
            return Vec::new();
        };
        entries.iter().rev().take(limit).cloned().collect()
    }

    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }
}

impl Default for RequestLog {
    fn default() -> Self {
        Self::new(1000)
    }
}

/// Split a request path into the bucket and key it addresses.
pub fn split_path(path: &str) -> (Option<String>, Option<String>) {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return (None, None);
    }
    match trimmed.split_once('/') {
        Some((bucket, key)) if !key.is_empty() => (Some(bucket.to_string()), Some(key.to_string())),
        Some((bucket, _)) => (Some(bucket.to_string()), None),
        None => (Some(trimmed.to_string()), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> RequestEntry {
        RequestEntry {
            at: Utc::now(),
            method: "GET".into(),
            path: path.into(),
            query: None,
            status: 200,
            duration_ms: 1,
            bucket: None,
            key: None,
            access_key_id: None,
            request_id: None,
        }
    }

    mod request_log {
        use super::*;

        #[test]
        fn should_return_entries_newest_first() {
            let log = RequestLog::new(10);
            log.record(entry("/first"));
            log.record(entry("/second"));

            let recent = log.recent(10);

            assert_eq!(recent[0].path, "/second");
            assert_eq!(recent[1].path, "/first");
        }

        #[test]
        fn should_drop_the_oldest_entry_once_full() {
            let log = RequestLog::new(2);
            for path in ["/one", "/two", "/three"] {
                log.record(entry(path));
            }

            let recent = log.recent(10);

            assert_eq!(recent.len(), 2);
            assert_eq!(recent[0].path, "/three");
            assert_eq!(recent[1].path, "/two");
        }

        #[test]
        fn should_record_nothing_when_disabled() {
            let log = RequestLog::new(0);
            log.record(entry("/ignored"));

            assert!(!log.is_enabled());
            assert!(log.recent(10).is_empty());
        }

        #[test]
        fn should_honour_the_requested_limit() {
            let log = RequestLog::new(10);
            for path in ["/one", "/two", "/three"] {
                log.record(entry(path));
            }

            assert_eq!(log.recent(2).len(), 2);
        }
    }

    mod split_path {
        use super::*;

        #[test]
        fn should_return_nothing_for_the_service_root() {
            assert_eq!(split_path("/"), (None, None));
        }

        #[test]
        fn should_read_a_bucket_on_its_own() {
            assert_eq!(split_path("/my-bucket"), (Some("my-bucket".into()), None));
        }

        #[test]
        fn should_read_a_bucket_and_key() {
            assert_eq!(
                split_path("/my-bucket/photos/cat.jpg"),
                (Some("my-bucket".into()), Some("photos/cat.jpg".into()))
            );
        }

        #[test]
        fn should_treat_a_trailing_slash_as_no_key() {
            assert_eq!(split_path("/my-bucket/"), (Some("my-bucket".into()), None));
        }
    }
}
