//! Pure output-path resolution: where a job's artifact lands under each
//! output target, and what makes a filename fragment legal.

use std::path::PathBuf;

use crfty_core::{ClaimedJob, OutputTarget};

const OUTPUT_CONTAINER_EXTENSION: &str = "mkv";

pub(super) fn resolve_output(
    job: &ClaimedJob,
) -> Result<(PathBuf, crfty_core::Replacement), String> {
    resolve_output_for(&job.spec.input, &job.spec.output_target)
}

fn resolve_output_for(
    input: &std::path::Path,
    output_target: &OutputTarget,
) -> Result<(PathBuf, crfty_core::Replacement), String> {
    output_target.validate().map_err(str::to_owned)?;
    let stem = input
        .file_stem()
        .ok_or_else(|| "input has no file stem".to_owned())?;
    match output_target {
        OutputTarget::Replace => {
            let is_matroska = input.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case(OUTPUT_CONTAINER_EXTENSION)
            });
            Ok((
                if is_matroska {
                    input.to_path_buf()
                } else {
                    input.with_extension(OUTPUT_CONTAINER_EXTENSION)
                },
                if is_matroska {
                    crfty_core::Replacement::KeepOriginal
                } else {
                    crfty_core::Replacement::RetireOriginal
                },
            ))
        }
        OutputTarget::Suffix { suffix } => {
            let mut name = stem.to_os_string();
            name.push(suffix);
            name.push(".");
            name.push(OUTPUT_CONTAINER_EXTENSION);
            validate_windows_file_name(&name)?;
            Ok((
                input.with_file_name(name),
                crfty_core::Replacement::KeepOriginal,
            ))
        }
        OutputTarget::SeparateFolder {
            directory,
            source_root,
        } => {
            let relative_parent = match source_root {
                Some(root) => input
                    .parent()
                    .ok_or_else(|| "input has no parent directory".to_owned())?
                    .strip_prefix(root)
                    .map_err(|_| "input is outside the configured source root".to_owned())?,
                None => std::path::Path::new(""),
            };
            if relative_parent
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err("source-relative output path is not safely contained".to_owned());
            }
            let parent = directory.join(relative_parent);
            Ok((
                parent.join(stem).with_extension(OUTPUT_CONTAINER_EXTENSION),
                crfty_core::Replacement::KeepOriginal,
            ))
        }
    }
}

fn validate_windows_file_name(name: &std::ffi::OsStr) -> Result<(), String> {
    let Some(name) = name.to_str() else {
        return Ok(());
    };
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|number| {
                number.len() == 1 && matches!(number.as_bytes().first(), Some(b'1'..=b'9'))
            });
    if reserved {
        Err("output filename is reserved by Windows".to_owned())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crfty_core::{OutputTarget, Replacement};

    use super::{resolve_output_for, validate_windows_file_name};

    #[test]
    fn suffix_is_a_filename_fragment_not_a_path() {
        for invalid in [
            "",
            "../escape",
            "\\escape",
            "bad:name",
            "bad?name",
            "trailing.",
            "space ",
        ] {
            assert!(
                OutputTarget::Suffix {
                    suffix: invalid.to_owned(),
                }
                .validate()
                .is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(
            OutputTarget::Suffix {
                suffix: "_av1".to_owned(),
            }
            .validate()
            .is_ok()
        );
        assert!(validate_windows_file_name(std::ffi::OsStr::new("CON.mkv")).is_err());
        assert!(validate_windows_file_name(std::ffi::OsStr::new("LPT9.mkv")).is_err());
    }

    #[test]
    fn replace_preserves_case_equivalent_mkv_path() {
        let input = Path::new("Movie.MKV");
        let (output, replacement) =
            resolve_output_for(input, &OutputTarget::Replace).expect("replace path");
        assert_eq!(output, input);
        assert_eq!(replacement, Replacement::KeepOriginal);
    }

    #[test]
    fn separate_output_rejects_input_outside_source_root() {
        let target = OutputTarget::SeparateFolder {
            directory: PathBuf::from("output"),
            source_root: Some(PathBuf::from("library")),
        };
        assert!(resolve_output_for(Path::new("elsewhere/movie.mkv"), &target).is_err());
    }
}
