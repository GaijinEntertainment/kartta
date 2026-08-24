//! Opening a path whose kind the index only knows from the walk.

use std::path::Path;
use std::{fs, io};

/// Whether an open may end on a symlink's target rather than on the named entry itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Symlinks {
    /// Resolve a final symlink, as git resolves one on an ignore file it was pointed at.
    Follow,
    /// Refuse a final symlink, so the open reaches the named entry or nothing.
    Refuse,
}

/// Opens `path` for reading and answers with the handle only when it turned out to be a regular file.
///
/// Every caller here holds a kind the walk read earlier, from a directory listing rather than from the file, and the
/// path may name something else by now. Three of those something-elses are hazards rather than errors: a fifo opened
/// for reading blocks until a writer appears, a directory or device node would be read as content, and a symlink would
/// carry the read outside the entry the caller asked about. So the open cannot wait, what it returns is classified
/// before a byte is read, and `symlinks` says whether a link may be resolved at all.
///
/// `O_NONBLOCK` is what keeps the open from waiting; it is a no-op for the regular file that is the only kind to
/// survive the check. Windows needs no such flag, because a named pipe lives in its own namespace rather than the
/// filesystem and a path there cannot reach one.
///
/// [`Symlinks::Refuse`] is honoured on unix, through `O_NOFOLLOW`. It is **not** honoured on Windows, where the open
/// resolves a link as it always has — so on that platform a regular file swapped for a link between the walk and the
/// read is still read through. Closing that needs the reparse-point flag and a Windows host to prove it on.
///
/// # Errors
///
/// Propagates the failure to open or to stat the handle, and reports [`io::ErrorKind::InvalidInput`] when the opened
/// handle is not a regular file.
pub(crate) fn open_regular(path: &Path, symlinks: Symlinks) -> io::Result<fs::File> {
    let file = open_guarded(path, symlinks)?;

    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }

    Ok(file)
}

/// Opens `path` for reading without waiting, and without resolving a final symlink unless allowed to.
#[cfg(unix)]
fn open_guarded(path: &Path, symlinks: Symlinks) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut flags = libc::O_NONBLOCK;
    if symlinks == Symlinks::Refuse {
        flags |= libc::O_NOFOLLOW;
    }

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
}

#[cfg(not(unix))]
fn open_guarded(path: &Path, _symlinks: Symlinks) -> io::Result<fs::File> {
    fs::File::open(path)
}
