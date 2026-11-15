//! CORS: the one bucket setting mock-s3 acts on rather than merely accepting.
//!
//! A browser cannot talk to the server at all without it. Anything uploading from a web
//! page sends a preflight `OPTIONS` first and refuses to continue unless the response
//! allows the origin, so a mock that ignores CORS is unusable from a browser however
//! well it serves everything else.
//!
//! Rules are stored as the client sent them and parsed on read — see
//! `mock_s3_core::bucket_config`.

use http::StatusCode;

use crate::error::S3Error;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CorsRule {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub expose_headers: Vec<String>,
    pub max_age_seconds: Option<u32>,
}

/// What a matching rule permits, ready to go on the wire.
#[derive(Debug, PartialEq, Eq)]
pub struct Allowance {
    pub origin: String,
    pub methods: String,
    pub headers: Option<String>,
    pub expose_headers: Option<String>,
    pub max_age_seconds: Option<u32>,
}

/// The error S3 returns when a CORS request is not allowed.
pub fn forbidden() -> S3Error {
    S3Error::new(
        StatusCode::FORBIDDEN,
        "AccessForbidden",
        "CORSResponse: This CORS request is not allowed. The CORS configuration is \
         missing, or does not allow this origin, method or header.",
    )
}

pub fn parse(body: &[u8]) -> Result<Vec<CorsRule>, S3Error> {
    let mut rules: Vec<CorsRule> = Vec::new();
    let mut current = CorsRule::default();

    crate::parse::walk(body, |element, text| match (element, text) {
        ("CORSRule", None) => current = CorsRule::default(),
        ("AllowedOrigin", Some(value)) => current.allowed_origins.push(value.trim().to_string()),
        ("AllowedMethod", Some(value)) => current
            .allowed_methods
            .push(value.trim().to_ascii_uppercase()),
        ("AllowedHeader", Some(value)) => current.allowed_headers.push(value.trim().to_string()),
        ("ExposeHeader", Some(value)) => current.expose_headers.push(value.trim().to_string()),
        ("MaxAgeSeconds", Some(value)) => current.max_age_seconds = value.trim().parse().ok(),
        ("/CORSRule", None) => rules.push(std::mem::take(&mut current)),
        _ => {}
    })?;

    if rules.is_empty() {
        return Err(S3Error::new(
            StatusCode::BAD_REQUEST,
            "MalformedXML",
            "A CORS configuration must contain at least one CORSRule.",
        ));
    }
    Ok(rules)
}

/// Does an `AllowedOrigin` pattern cover this origin?
///
/// S3 allows at most one `*` in a pattern, standing for any sequence of characters, so
/// `https://*.example.com` is meaningful and is what people actually write.
pub fn origin_matches(pattern: &str, origin: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.split_once('*') {
        None => pattern == origin,
        Some((before, after)) => {
            origin.len() >= before.len() + after.len()
                && origin.starts_with(before)
                && origin.ends_with(after)
        }
    }
}

fn header_allowed(rule: &CorsRule, requested: &str) -> bool {
    rule.allowed_headers.iter().any(|allowed| {
        allowed == "*" || allowed.eq_ignore_ascii_case(requested) || {
            // `*` may also appear inside a pattern, the same as for origins.
            allowed.contains('*')
                && origin_matches(
                    &allowed.to_ascii_lowercase(),
                    &requested.to_ascii_lowercase(),
                )
        }
    })
}

/// Find the rule that permits this request, if any.
///
/// `requested_headers` is the comma-separated `Access-Control-Request-Headers` value on
/// a preflight, and empty for an actual request.
pub fn evaluate(
    rules: &[CorsRule],
    origin: &str,
    method: &str,
    requested_headers: &str,
) -> Option<Allowance> {
    let method = method.to_ascii_uppercase();
    let wanted: Vec<&str> = requested_headers
        .split(',')
        .map(str::trim)
        .filter(|header| !header.is_empty())
        .collect();

    let (rule, matched) = rules.iter().find_map(|rule| {
        let matched = rule
            .allowed_origins
            .iter()
            .find(|pattern| origin_matches(pattern, origin))?;

        let usable = rule.allowed_methods.contains(&method)
            && wanted.iter().all(|header| header_allowed(rule, header));
        usable.then_some((rule, matched))
    })?;

    Some(Allowance {
        // A rule of `*` is echoed as `*`, and any other pattern is echoed as the concrete
        // origin it matched. Echoing the origin for `*` as well would look more useful —
        // it is what a request carrying credentials needs — but S3 returns the wildcard,
        // and a browser that caches `*` behaves differently from one that caches an
        // origin. Matching S3 is the whole point.
        origin: if matched == "*" {
            "*".to_string()
        } else {
            origin.to_string()
        },
        methods: rule.allowed_methods.join(", "),
        headers: (!wanted.is_empty()).then(|| wanted.join(", ")),
        expose_headers: (!rule.expose_headers.is_empty()).then(|| rule.expose_headers.join(", ")),
        max_age_seconds: rule.max_age_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(origins: &[&str], methods: &[&str], headers: &[&str]) -> CorsRule {
        CorsRule {
            allowed_origins: origins.iter().map(|s| s.to_string()).collect(),
            allowed_methods: methods.iter().map(|s| s.to_uppercase()).collect(),
            allowed_headers: headers.iter().map(|s| s.to_string()).collect(),
            expose_headers: Vec::new(),
            max_age_seconds: None,
        }
    }

    mod origin_matches {
        use super::*;

        #[test]
        fn should_match_a_star() {
            assert!(origin_matches("*", "https://example.com"));
        }

        #[test]
        fn should_match_exactly() {
            assert!(origin_matches("https://example.com", "https://example.com"));
            assert!(!origin_matches("https://example.com", "https://other.com"));
        }

        #[test]
        fn should_match_a_wildcard_subdomain() {
            assert!(origin_matches(
                "https://*.example.com",
                "https://app.example.com"
            ));
            assert!(!origin_matches(
                "https://*.example.com",
                "https://example.org"
            ));
        }

        #[test]
        fn should_not_let_a_wildcard_match_a_shorter_origin() {
            assert!(!origin_matches(
                "https://*.example.com",
                "https://.example.co"
            ));
        }
    }

    mod parse {
        use super::*;

        #[test]
        fn should_read_a_rule() {
            let body = br#"<CORSConfiguration><CORSRule>
                <AllowedOrigin>https://example.com</AllowedOrigin>
                <AllowedMethod>get</AllowedMethod>
                <AllowedMethod>PUT</AllowedMethod>
                <AllowedHeader>*</AllowedHeader>
                <ExposeHeader>ETag</ExposeHeader>
                <MaxAgeSeconds>3000</MaxAgeSeconds>
            </CORSRule></CORSConfiguration>"#;

            let rules = parse(body).unwrap();

            assert_eq!(rules.len(), 1);
            assert_eq!(rules[0].allowed_origins, vec!["https://example.com"]);
            // Methods are upper-cased, so a lower-case rule still matches a request.
            assert_eq!(rules[0].allowed_methods, vec!["GET", "PUT"]);
            assert_eq!(rules[0].expose_headers, vec!["ETag"]);
            assert_eq!(rules[0].max_age_seconds, Some(3000));
        }

        #[test]
        fn should_read_several_rules() {
            let body = br#"<CORSConfiguration>
                <CORSRule><AllowedOrigin>a</AllowedOrigin><AllowedMethod>GET</AllowedMethod></CORSRule>
                <CORSRule><AllowedOrigin>b</AllowedOrigin><AllowedMethod>PUT</AllowedMethod></CORSRule>
            </CORSConfiguration>"#;

            let rules = parse(body).unwrap();

            assert_eq!(rules.len(), 2);
            assert_eq!(rules[1].allowed_methods, vec!["PUT"]);
        }

        #[test]
        fn should_reject_a_configuration_with_no_rules() {
            let error = parse(b"<CORSConfiguration></CORSConfiguration>").unwrap_err();

            assert_eq!(error.code, "MalformedXML");
        }
    }

    mod evaluate {
        use super::*;

        #[test]
        fn should_allow_a_matching_origin_and_method() {
            let rules = vec![rule(&["https://example.com"], &["GET"], &[])];

            let allowed = evaluate(&rules, "https://example.com", "GET", "").unwrap();

            assert_eq!(allowed.origin, "https://example.com");
            assert_eq!(allowed.methods, "GET");
        }

        #[test]
        fn should_refuse_an_origin_no_rule_covers() {
            let rules = vec![rule(&["https://example.com"], &["GET"], &[])];

            assert!(evaluate(&rules, "https://evil.com", "GET", "").is_none());
        }

        /// A rule of `*` is echoed as `*`, not as the origin that matched it. Echoing
        /// the origin would be more useful to a credentialed request, and S3 does not do
        /// it — a browser caches the two differently.
        #[test]
        fn should_echo_the_wildcard_rather_than_the_origin_for_a_wildcard_rule() {
            let rules = vec![rule(&["*"], &["GET"], &[])];

            let allowed = evaluate(&rules, "https://example.com", "GET", "").unwrap();

            assert_eq!(allowed.origin, "*");
        }

        /// Any other pattern, wildcards included, is echoed as the concrete origin.
        #[test]
        fn should_echo_the_concrete_origin_for_a_partial_wildcard() {
            let rules = vec![rule(&["https://*.example.com"], &["GET"], &[])];

            let allowed = evaluate(&rules, "https://app.example.com", "GET", "").unwrap();

            assert_eq!(allowed.origin, "https://app.example.com");
        }

        #[test]
        fn should_refuse_a_method_the_rule_does_not_list() {
            let rules = vec![rule(&["*"], &["GET"], &[])];

            assert!(evaluate(&rules, "https://example.com", "DELETE", "").is_none());
        }

        #[test]
        fn should_match_the_method_case_insensitively() {
            let rules = vec![rule(&["*"], &["put"], &[])];

            assert!(evaluate(&rules, "https://example.com", "put", "").is_some());
        }

        #[test]
        fn should_refuse_a_requested_header_the_rule_does_not_allow() {
            let rules = vec![rule(&["*"], &["PUT"], &["content-type"])];

            assert!(evaluate(&rules, "https://example.com", "PUT", "x-amz-acl").is_none());
        }

        #[test]
        fn should_allow_any_requested_header_when_the_rule_says_star() {
            let rules = vec![rule(&["*"], &["PUT"], &["*"])];

            let allowed = evaluate(
                &rules,
                "https://example.com",
                "PUT",
                "x-amz-acl, content-type",
            )
            .unwrap();

            assert_eq!(allowed.headers.as_deref(), Some("x-amz-acl, content-type"));
        }

        #[test]
        fn should_pick_the_first_rule_that_fits() {
            let rules = vec![
                rule(&["https://other.com"], &["GET"], &[]),
                rule(&["https://example.com"], &["GET", "PUT"], &[]),
            ];

            let allowed = evaluate(&rules, "https://example.com", "PUT", "").unwrap();

            assert_eq!(allowed.methods, "GET, PUT");
        }
    }
}
