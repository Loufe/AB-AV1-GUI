use std::{io, path::Path};

pub(crate) fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(unix)]
pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    std::fs::File::open(parent_directory(path))?.sync_all()
}

#[cfg(not(unix))]
pub(crate) fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::parent_directory;

    #[test]
    fn relative_file_uses_current_directory_as_parent() {
        assert_eq!(parent_directory(Path::new("config.json")), Path::new("."));
    }
}
