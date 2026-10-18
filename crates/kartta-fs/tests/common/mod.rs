use std::fs;
use std::path::Path;

use tempfile::TempDir;

/// A fresh temporary directory holding `spec`: each path a file with its content, or a directory when it ends in `/`.
pub(crate) fn tree(spec: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    for (path, content) in spec {
        match path.strip_suffix('/') {
            Some(name) => fs::create_dir_all(dir.path().join(name)).expect("create directory"),
            None => write(dir.path(), path, content),
        }
    }
    dir
}

pub(crate) fn write(root: &Path, path: &str, content: impl AsRef<[u8]>) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(&full, content).expect("write file");
}
