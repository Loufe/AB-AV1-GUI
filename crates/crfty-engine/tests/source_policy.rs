#![forbid(unsafe_code)]

//! Tidy-style source policy for the whole workspace (AGENTS.md "Comments"):
//! comment text must not carry GitHub issue references, and no source file may
//! open with a file-path header comment. Knowledge belongs in the repo; the
//! tracker is coordination only.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

const SKIP_DIRS: [&str; 4] = ["target", "node_modules", "dist", ".git"];
// Generated files are exempt: their comments are not hand-written.
const SKIP_FILES: [&str; 1] = ["bindings.ts"];

fn collect_sources(root: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                collect_sources(&path, out)?;
            }
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("rs" | "ts" | "tsx")
        ) && !SKIP_FILES.contains(&name.as_str())
        {
            out.push(path);
        }
    }
    Ok(())
}

/// The comment portion of a line: the whole line for `//`-, `/*`-, and
/// `*`-led lines, otherwise the tail of a trailing `//` comment, if any.
fn comment_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
        return Some(trimmed);
    }
    line.find("//").and_then(|pos| line.get(pos..))
}

/// `#` followed by one to five digits ending at a word boundary: the shape of
/// a tracker reference in any phrasing (`(#NN)`, `issue #NN`, `per #NN`,
/// bare `#NN`). Attributes (`#[...]`) have no digits, HTML entities
/// (`&#39;`) are excluded by the `&` guard, and six-digit hex colors exceed
/// the digit cap; three-digit hex shorthand in a comment is accepted as a
/// false positive worth spelling out.
fn has_issue_ref(comment: &str) -> bool {
    let bytes = comment.as_bytes();
    bytes.iter().enumerate().any(|(pos, &byte)| {
        if byte != b'#' {
            return false;
        }
        if pos > 0 && bytes.get(pos.wrapping_sub(1)) == Some(&b'&') {
            return false;
        }
        let digits = bytes
            .get(pos + 1..)
            .unwrap_or_default()
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if digits == 0 || digits > 5 {
            return false;
        }
        !bytes
            .get(pos + 1 + digits)
            .is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

fn is_path_header(first_line: &str) -> bool {
    let trimmed = first_line.trim_start();
    if !trimmed.starts_with("//") {
        return false;
    }
    let body = trimmed.trim_start_matches(['/', '!', '*']).trim_start();
    ["crates/", "ui/", "src/"]
        .iter()
        .any(|prefix| body.starts_with(prefix))
}

#[test]
fn workspace_comments_follow_source_policy() -> io::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut sources = Vec::new();
    collect_sources(&root.join("crates"), &mut sources)?;
    collect_sources(&root.join("ui").join("src"), &mut sources)?;
    assert!(!sources.is_empty(), "source walk found no files");

    let mut violations = Vec::new();
    for path in &sources {
        let text = fs::read_to_string(path)?;
        for (index, line) in text.lines().enumerate() {
            let Some(comment) = comment_text(line) else {
                continue;
            };
            if has_issue_ref(comment) {
                violations.push(format!(
                    "{}:{}: issue reference in comment",
                    path.display(),
                    index + 1
                ));
            }
            if index == 0 && is_path_header(line) {
                violations.push(format!("{}:1: file-path header comment", path.display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "source policy violations (AGENTS.md \"Comments\"):\n{}",
        violations.join("\n")
    );
    Ok(())
}
