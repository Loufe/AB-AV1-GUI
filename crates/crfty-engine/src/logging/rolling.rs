//! Pure naming and retention logic for the log sink: per-launch file names,
//! size-triggered rotation plans, and old-launch pruning. The sink in
//! `logging::mod` executes these plans; everything here is a pure function.

use std::path::{Path, PathBuf};

/// Rotate the active file once it reaches this size (V2's
//  `RotatingFileHandler` cap).
pub(crate) const FILE_CAP_BYTES: u64 = 10 * 1024 * 1024;
/// Rolled files kept per launch (V2's `backupCount`).
pub(crate) const ROLLED_KEEP: u32 = 5;
/// Launches whose files survive pruning. V2 never pruned old launch files;
/// bounding them is a deliberate V3 improvement.
pub(crate) const LAUNCH_KEEP: usize = 10;

const LOG_FILE_PREFIX: &str = "crfty_";
const LOG_FILE_SUFFIX: &str = ".log";

/// `crfty_YYYY-MM-DD_HH-MM-SS.log` for the launch instant (UTC). The name
/// sorts lexicographically in chronological order, which pruning relies on.
pub(crate) fn launch_file_name(unix_seconds: u64) -> String {
    let (year, month, day, hour, minute, second) = civil_from_unix(unix_seconds);
    format!(
        "{LOG_FILE_PREFIX}{year:04}-{month:02}-{day:02}_{hour:02}-{minute:02}-{second:02}{LOG_FILE_SUFFIX}"
    )
}

/// `crfty_<ts>.log` → `crfty_<ts>.N.log`.
pub(crate) fn rolled_file_name(base_name: &str, index: u32) -> String {
    let stem = base_name.strip_suffix(LOG_FILE_SUFFIX).unwrap_or(base_name);
    format!("{stem}.{index}{LOG_FILE_SUFFIX}")
}

/// The rename cascade executed when the active file reaches the cap:
/// `.4 → .5`, …, `base → .1`, oldest-first so nothing is overwritten. The
/// caller deletes the returned `discard` first (the previous `.5`).
pub(crate) struct RotationPlan {
    pub(crate) discard: PathBuf,
    /// (from, to), safe to apply in order.
    pub(crate) renames: Vec<(PathBuf, PathBuf)>,
}

pub(crate) fn rotation_plan(directory: &Path, base_name: &str) -> RotationPlan {
    let rolled = |index: u32| directory.join(rolled_file_name(base_name, index));
    let mut renames = Vec::new();
    for index in (1..ROLLED_KEEP).rev() {
        renames.push((rolled(index), rolled(index + 1)));
    }
    renames.push((directory.join(base_name), rolled(1)));
    RotationPlan {
        discard: rolled(ROLLED_KEEP),
        renames,
    }
}

/// Given the log directory's `crfty_*.log` file names, returns the ones to
/// delete: every file whose launch (the name up to the first `.`) is older
/// than the newest [`LAUNCH_KEEP`] launches. Rolled files follow their base.
pub(crate) fn prune_selection(file_names: &[String]) -> Vec<String> {
    let mut launches: Vec<&str> = file_names
        .iter()
        .filter(|name| name.starts_with(LOG_FILE_PREFIX) && name.ends_with(LOG_FILE_SUFFIX))
        .map(|name| launch_key(name))
        .collect();
    launches.sort_unstable();
    launches.dedup();
    if launches.len() <= LAUNCH_KEEP {
        return Vec::new();
    }
    let cutoff = launches.len() - LAUNCH_KEEP;
    let expired: Vec<&str> = launches.drain(..cutoff).collect();
    file_names
        .iter()
        .filter(|name| name.starts_with(LOG_FILE_PREFIX) && name.ends_with(LOG_FILE_SUFFIX))
        .filter(|name| expired.contains(&launch_key(name)))
        .cloned()
        .collect()
}

/// `crfty_<ts>.log` and `crfty_<ts>.N.log` share the key `crfty_<ts>`.
fn launch_key(file_name: &str) -> &str {
    file_name
        .split_once('.')
        .map_or(file_name, |(key, _rest)| key)
}

/// Civil UTC date/time from a Unix timestamp (Howard Hinnant's
/// `civil_from_days`); valid for any post-epoch instant.
fn civil_from_unix(unix_seconds: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = unix_seconds / 86_400;
    let seconds_of_day = unix_seconds % 86_400;
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (
        year,
        month,
        day,
        seconds_of_day / 3_600,
        (seconds_of_day / 60) % 60,
        seconds_of_day % 60,
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{LAUNCH_KEEP, launch_file_name, prune_selection, rolled_file_name, rotation_plan};

    /// Vectors from `datetime.datetime.fromtimestamp(ts, timezone.utc)`.
    #[test]
    fn launch_names_match_the_civil_calendar() {
        assert_eq!(launch_file_name(0), "crfty_1970-01-01_00-00-00.log");
        // Leap-day and leap-century coverage.
        assert_eq!(
            launch_file_name(951_867_022),
            "crfty_2000-02-29_23-30-22.log"
        );
        assert_eq!(
            launch_file_name(1_752_998_400),
            "crfty_2025-07-20_08-00-00.log"
        );
        assert_eq!(
            launch_file_name(4_102_444_799),
            "crfty_2099-12-31_23-59-59.log"
        );
    }

    #[test]
    fn launch_names_sort_chronologically() {
        let earlier = launch_file_name(1_752_998_400);
        let later = launch_file_name(1_753_001_234);
        assert!(earlier < later);
    }

    #[test]
    fn rotation_shifts_every_rolled_file_without_overwriting() {
        let plan = rotation_plan(Path::new("logs"), "crfty_2025-07-20_08-00-00.log");
        assert_eq!(
            plan.discard,
            Path::new("logs").join("crfty_2025-07-20_08-00-00.5.log")
        );
        let names: Vec<(String, String)> = plan
            .renames
            .iter()
            .map(|(from, to)| {
                (
                    from.file_name().unwrap().to_string_lossy().into_owned(),
                    to.file_name().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                (
                    "crfty_2025-07-20_08-00-00.4.log",
                    "crfty_2025-07-20_08-00-00.5.log"
                ),
                (
                    "crfty_2025-07-20_08-00-00.3.log",
                    "crfty_2025-07-20_08-00-00.4.log"
                ),
                (
                    "crfty_2025-07-20_08-00-00.2.log",
                    "crfty_2025-07-20_08-00-00.3.log"
                ),
                (
                    "crfty_2025-07-20_08-00-00.1.log",
                    "crfty_2025-07-20_08-00-00.2.log"
                ),
                (
                    "crfty_2025-07-20_08-00-00.log",
                    "crfty_2025-07-20_08-00-00.1.log"
                ),
            ]
            .map(|(from, to)| (from.to_owned(), to.to_owned()))
        );
    }

    #[test]
    fn rolled_names_insert_the_index_before_the_suffix() {
        assert_eq!(
            rolled_file_name("crfty_2025-07-20_08-00-00.log", 3),
            "crfty_2025-07-20_08-00-00.3.log"
        );
    }

    #[test]
    fn pruning_keeps_the_newest_launches_and_their_rolled_files() {
        let mut names: Vec<String> = (0..LAUNCH_KEEP as u64 + 2)
            .map(|day| format!("crfty_2025-07-{:02}_00-00-00.log", day + 1))
            .collect();
        names.push("crfty_2025-07-01_00-00-00.1.log".to_owned());
        names.push("unrelated.txt".to_owned());
        let deletions = prune_selection(&names);
        assert_eq!(
            deletions,
            [
                "crfty_2025-07-01_00-00-00.log",
                "crfty_2025-07-02_00-00-00.log",
                "crfty_2025-07-01_00-00-00.1.log",
            ]
        );
    }

    #[test]
    fn pruning_is_a_no_op_within_the_launch_budget() {
        let names = vec![
            "crfty_2025-07-01_00-00-00.log".to_owned(),
            "crfty_2025-07-02_00-00-00.log".to_owned(),
        ];
        assert!(prune_selection(&names).is_empty());
    }
}
