//! Opening a path whose kind the index only knows from the walk.

use std::path::Path;
use std::{fs, io};

/// Opens `path` for reading and answers with the handle only when it turned out to be a regular file.
///
/// Every caller here holds a kind the walk read earlier, from a directory listing rather than from the file. The path
/// may name something else by now, and two of those something-elses are hazards rather than errors: a fifo opened for
/// reading blocks until a writer appears, which would hang the caller indefinitely, and a directory or device node
/// would be read as content. So the open cannot wait, and what it returns is classified before a byte is read.
///
/// `O_NONBLOCK` is what keeps the open from waiting; it is a no-op for the regular file that is the only kind to
/// survive the check. Windows needs no flag, because a named pipe lives in its own namespace rather than the
/// filesystem and a path there cannot reach one.
///
/// # Errors
///
/// Propagates the failure to open or to stat the handle, and reports [`io::ErrorKind::InvalidInput`] when the opened
/// handle is not a regular file.
pub(crate) fn open_regular(path: &Path) -> io::Result<fs::File> {
    let file = open_without_blocking(path)?;

    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }

    Ok(file)
}

/// Opens `path` for reading without waiting for anything.
#[cfg(unix)]
fn open_without_blocking(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_without_blocking(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}
