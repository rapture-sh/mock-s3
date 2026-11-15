use percent_encoding::percent_decode_str;

/// A parsed query string.
///
/// Hand-rolled because S3 uses valueless subresource parameters — `?uploads`,
/// `?versioning`, `?location` — which most form decoders either reject or turn into
/// something awkward, and because the order of repeated keys never matters here.
#[derive(Debug, Default)]
pub struct Query(Vec<(String, String)>);

impl Query {
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(raw) = raw else {
            return Self::default();
        };

        let pairs = raw
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) => (decode(key), decode(value)),
                None => (decode(pair), String::new()),
            })
            .collect();

        Self(pairs)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// Present at all, with or without a value. This is how a subresource is detected.
    pub fn has(&self, key: &str) -> bool {
        self.0.iter().any(|(name, _)| name == key)
    }

    /// A value that is present and not empty.
    pub fn non_empty(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|value| !value.is_empty())
    }
}

fn decode(raw: &str) -> String {
    // `+` means space in a query string; a literal plus arrives as %2B.
    let spaced = raw.replace('+', " ");
    percent_decode_str(&spaced).decode_utf8_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse {
        use super::*;

        #[test]
        fn should_return_nothing_for_an_absent_query_string() {
            let query = Query::parse(None);

            assert!(query.get("prefix").is_none());
        }

        #[test]
        fn should_read_a_plain_pair() {
            let query = Query::parse(Some("prefix=photos/"));

            assert_eq!(query.get("prefix"), Some("photos/"));
        }

        #[test]
        fn should_treat_a_valueless_parameter_as_present() {
            let query = Query::parse(Some("versioning"));

            assert!(query.has("versioning"));
            assert_eq!(query.get("versioning"), Some(""));
        }

        #[test]
        fn should_percent_decode_a_value() {
            let query = Query::parse(Some("prefix=a%2Fb%20c"));

            assert_eq!(query.get("prefix"), Some("a/b c"));
        }

        #[test]
        fn should_decode_a_plus_as_a_space() {
            let query = Query::parse(Some("prefix=a+b"));

            assert_eq!(query.get("prefix"), Some("a b"));
        }

        #[test]
        fn should_ignore_an_empty_pair() {
            let query = Query::parse(Some("a=1&&b=2"));

            assert_eq!(query.get("b"), Some("2"));
        }

        #[test]
        fn should_report_an_empty_value_as_not_non_empty() {
            let query = Query::parse(Some("delimiter="));

            assert!(query.has("delimiter"));
            assert!(query.non_empty("delimiter").is_none());
        }
    }
}
