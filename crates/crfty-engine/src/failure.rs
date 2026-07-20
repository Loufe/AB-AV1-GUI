//! Diagnostic capture for durable failures: path scrubbing happens here, at
//! the moment of capture, so the journal never records a real path. The
//! retroactive scrub tooling is the backstop, not the mechanism.

use std::path::Path;

use crfty_core::DiagnosticTail;

/// Substitutes each known run path with its placeholder, then bounds the
/// result. Longer path strings are replaced first so a path that contains
/// another (staging files typically extend the output path) scrubs fully
/// instead of leaving a recognizable suffix.
pub(crate) fn scrub_tail(text: &str, paths: &[(&Path, &str)]) -> DiagnosticTail {
    let mut replacements: Vec<(String, &str)> = paths
        .iter()
        .filter_map(|(path, placeholder)| {
            let rendered = path.to_string_lossy().into_owned();
            if rendered.is_empty() {
                None
            } else {
                Some((rendered, *placeholder))
            }
        })
        .collect();
    replacements.sort_by_key(|(rendered, _)| std::cmp::Reverse(rendered.len()));
    let mut scrubbed = text.to_owned();
    for (rendered, placeholder) in replacements {
        scrubbed = scrubbed.replace(&rendered, placeholder);
    }
    DiagnosticTail::truncated(&scrubbed)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::scrub_tail;

    #[test]
    fn replaces_each_known_path_with_its_placeholder() {
        let tail = scrub_tail(
            "Error opening /videos/movie.mp4; writing /videos/movie.mkv.part failed",
            &[
                (Path::new("/videos/movie.mp4"), "<input>"),
                (Path::new("/videos/movie.mkv.part"), "<staging>"),
                (Path::new("/videos/movie.mkv"), "<output>"),
            ],
        );
        assert_eq!(
            tail.as_str(),
            "Error opening <input>; writing <staging> failed"
        );
    }

    #[test]
    fn scrubs_the_longer_containing_path_before_its_prefix() {
        // The staging path extends the output path; replacing the shorter one
        // first would leave "<output>.part" instead of "<staging>".
        let tail = scrub_tail(
            "cannot rename /out/file.mkv.part to /out/file.mkv",
            &[
                (Path::new("/out/file.mkv"), "<output>"),
                (Path::new("/out/file.mkv.part"), "<staging>"),
            ],
        );
        assert_eq!(tail.as_str(), "cannot rename <staging> to <output>");
    }

    #[test]
    fn ignores_empty_paths_and_bounds_the_result() {
        let long = "x".repeat(crfty_core::DIAGNOSTIC_TAIL_MAX_BYTES + 100);
        let tail = scrub_tail(&long, &[(Path::new(""), "<input>")]);
        assert_eq!(tail.as_str().len(), crfty_core::DIAGNOSTIC_TAIL_MAX_BYTES);
        assert!(!tail.as_str().contains("<input>"));
    }
}
