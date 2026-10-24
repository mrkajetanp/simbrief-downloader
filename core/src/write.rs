//! Atomic file writes: readers see either the old file or the complete new one.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Writes `contents` to `path` via a temp file in the same directory, `fsync`, then rename.
/// The temp file is removed if anything fails.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = temp_path(path)?;
    let result = write_and_rename(&tmp, path, contents);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn temp_path(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("'{}' has no file name", path.display()),
        )
    })?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(".tmp");
    Ok(path.with_file_name(tmp_name))
}

fn write_and_rename(tmp: &Path, path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = File::create(tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writes_new_file_without_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("KJFKEGLL.fms");
        atomic_write(&path, b"plan").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"plan");
        assert_eq!(entries(dir.path()), vec!["KJFKEGLL.fms"]);
    }

    #[test]
    fn overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("KJFKEGLL.fms");
        fs::write(&path, b"old plan that is longer").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(entries(dir.path()), vec!["KJFKEGLL.fms"]);
    }

    #[test]
    fn fails_cleanly_when_directory_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("KJFKEGLL.fms");
        assert!(atomic_write(&path, b"plan").is_err());
        assert!(entries(dir.path()).is_empty());
    }

    #[test]
    fn removes_temp_file_when_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        // A non-empty directory in the way makes the rename fail after the temp file exists.
        let path = dir.path().join("KJFKEGLL.fms");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"x").unwrap();
        assert!(atomic_write(&path, b"plan").is_err());
        assert_eq!(entries(dir.path()), vec!["KJFKEGLL.fms"]);
    }
}
