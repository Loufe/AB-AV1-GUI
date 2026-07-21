//! Manual application release check against the GitHub releases API.
//!
//! One-shot and user-initiated only: there is no background checking and no
//! setting that enables it (#33 §12). The comparison ports V2's semantics
//! exactly — dot-separated numeric tuples, falling back to string equality
//! when either side does not parse (which is every pre-release, so an alpha
//! reports any differently-tagged release as an update).

use std::io::Read;

use serde::Deserialize;

use crate::vendor::download::{Fetch, HttpFetch};

/// The V2 repository; #45 renames it for CRFty.
const RELEASE_API_URL: &str = "https://api.github.com/repos/Loufe/AB-AV1-GUI/releases/latest";
/// Far above any real release payload; bounds a misbehaving server.
const RELEASE_BODY_CAP_BYTES: u64 = 1024 * 1024;

/// Outcome of a successful check. `html_url` is the release page GitHub
/// reported — the shell keeps it engine-side of the webview and opens it on
/// request rather than letting the frontend pass URLs around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseCheck {
    pub current: String,
    pub latest: String,
    pub update_available: bool,
    pub html_url: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    html_url: String,
}

/// Fetches the latest release and compares it against `current_version`.
/// Blocks for the duration of the request — call off the UI thread.
pub fn check_latest_release(current_version: &str) -> Result<ReleaseCheck, String> {
    let fetch = HttpFetch::new()?;
    check_latest_release_with(&fetch, current_version)
}

fn check_latest_release_with(fetch: &dyn Fetch, current: &str) -> Result<ReleaseCheck, String> {
    let stream = fetch
        .fetch(RELEASE_API_URL)
        .map_err(|error| format!("the update check failed: {error}"))?;
    let mut body = String::new();
    stream
        .reader
        .take(RELEASE_BODY_CAP_BYTES)
        .read_to_string(&mut body)
        .map_err(|error| format!("failed to read the GitHub response: {error}"))?;
    let release: LatestRelease = serde_json::from_str(&body)
        .map_err(|error| format!("failed to parse the GitHub response: {error}"))?;
    // V2 stripped every leading 'v' (str.lstrip), so tags like "v1.2.3"
    // and the hypothetical "vv1.2.3" normalize identically.
    let latest = release.tag_name.trim_start_matches('v');
    if latest.is_empty() {
        return Err("could not read a version from the GitHub response".to_owned());
    }
    Ok(ReleaseCheck {
        current: current.to_owned(),
        latest: latest.to_owned(),
        update_available: !is_up_to_date(current, latest),
        html_url: release.html_url,
    })
}

/// V2's comparison: numeric dot-tuples, current >= latest means up to date;
/// if either side fails to parse, only exact string equality is up to date.
fn is_up_to_date(current: &str, latest: &str) -> bool {
    match (parse_dotted(current), parse_dotted(latest)) {
        (Some(current_parts), Some(latest_parts)) => current_parts >= latest_parts,
        _ => current == latest,
    }
}

fn parse_dotted(version: &str) -> Option<Vec<u64>> {
    version
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::vendor::download::{Fetch, FetchStream};

    use super::{check_latest_release_with, is_up_to_date};

    struct StaticFetch(&'static str);

    impl Fetch for StaticFetch {
        fn fetch(&self, _url: &str) -> Result<FetchStream, String> {
            Ok(FetchStream {
                total: None,
                reader: Box::new(Cursor::new(self.0.as_bytes())),
            })
        }
    }

    struct FailingFetch;

    impl Fetch for FailingFetch {
        fn fetch(&self, _url: &str) -> Result<FetchStream, String> {
            Err("the download server answered 403 Forbidden".to_owned())
        }
    }

    #[test]
    fn numeric_versions_compare_as_tuples() {
        assert!(is_up_to_date("2.0.0", "2.0.0"));
        assert!(is_up_to_date("2.1.0", "2.0.9"));
        assert!(!is_up_to_date("2.0.9", "2.1.0"));
        // Python tuple ordering: a shorter equal prefix is older.
        assert!(!is_up_to_date("1.2", "1.2.3"));
        assert!(is_up_to_date("1.2.3", "1.2"));
    }

    #[test]
    fn unparsable_versions_fall_back_to_string_equality() {
        assert!(is_up_to_date("3.0.0-alpha.0", "3.0.0-alpha.0"));
        assert!(!is_up_to_date("3.0.0-alpha.0", "3.0.0"));
        assert!(!is_up_to_date("dev", "2.0.0"));
    }

    #[test]
    fn tags_lose_their_v_prefix_and_compare_against_current() {
        let fetch =
            StaticFetch(r#"{"tag_name": "v2.1.0", "html_url": "https://example.invalid/rel"}"#);
        let check = check_latest_release_with(&fetch, "2.0.0").expect("check succeeds");
        assert_eq!(check.latest, "2.1.0");
        assert_eq!(check.current, "2.0.0");
        assert!(check.update_available);
        assert_eq!(check.html_url, "https://example.invalid/rel");
    }

    #[test]
    fn an_up_to_date_current_version_reports_no_update() {
        let fetch = StaticFetch(r#"{"tag_name": "v2.1.0", "html_url": "u"}"#);
        let check = check_latest_release_with(&fetch, "2.1.0").expect("check succeeds");
        assert!(!check.update_available);
    }

    #[test]
    fn a_missing_tag_is_an_error() {
        let fetch = StaticFetch(r#"{"html_url": "u"}"#);
        assert!(check_latest_release_with(&fetch, "2.0.0").is_err());
    }

    #[test]
    fn malformed_json_is_an_error() {
        let fetch = StaticFetch("not json");
        assert!(check_latest_release_with(&fetch, "2.0.0").is_err());
    }

    #[test]
    fn transport_failures_surface_as_errors() {
        let error = check_latest_release_with(&FailingFetch, "2.0.0")
            .expect_err("transport failure propagates");
        assert!(error.contains("403"));
    }
}
