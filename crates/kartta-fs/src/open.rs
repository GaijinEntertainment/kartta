use std::path::Path;
use std::{fs, io};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Symlinks {
    Follow,
    Refuse,
}

pub(crate) fn open_regular(
    path: &Path,
    symlinks: Symlinks,
) -> io::Result<(fs::File, fs::Metadata)> {
    let file = open_guarded(path, symlinks)?;
    let metadata = file.metadata()?;

    // constraint: the kind is settled on the opened handle — a stat of the name and an open of it resolve the name
    // twice, and a fifo renamed onto it in between blocks the read
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }

    Ok((file, metadata))
}

#[cfg(unix)]
fn open_guarded(path: &Path, symlinks: Symlinks) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    // constraint: a reading open of a fifo waits for a writer unless O_NONBLOCK, which changes nothing for the regular
    // file that is the only kind let through
    let mut flags = libc::O_NONBLOCK;
    if symlinks == Symlinks::Refuse {
        flags |= libc::O_NOFOLLOW;
    }

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
}

// constraint: a Windows named pipe lives in its own namespace, so no filesystem path reaches one and no flag is needed
// to keep the open from waiting
// shortcut: Windows resolves a final symlink even under `Symlinks::Refuse` — open with the reparse-point flag once a
// Windows host can prove it
#[cfg(not(unix))]
fn open_guarded(path: &Path, _symlinks: Symlinks) -> io::Result<fs::File> {
    fs::File::open(path)
}
