//! What a refresh compares to tell a file it can carry over from one it must read again.

use std::fs::Metadata;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use kartta_walk::ListedStat;

pub(crate) type Stamp = u64;

pub(crate) const UNTRUSTED: Stamp = 0;
const VISITED: Stamp = 1 << 63;

// constraint: a filesystem whose clock ticks coarsely gives a write landing after the read the time the read saw, so
// a file changed this close to the walk's start is read again rather than trusted
const RACY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
pub(crate) struct Stamping {
    trusted_before: u64,
}

impl Stamping {
    pub(crate) fn starting_now() -> Self {
        let racy = u64::try_from(RACY.as_nanos()).unwrap_or(u64::MAX);
        let now = nanos(SystemTime::now()).unwrap_or(0);

        Self {
            trusted_before: now.saturating_sub(racy),
        }
    }

    pub(crate) fn stamp(self, metadata: &Metadata, visited: bool) -> Stamp {
        match changed(metadata) {
            Some(time) if time != UNTRUSTED && time < self.trusted_before && time < VISITED => {
                if visited {
                    time | VISITED
                } else {
                    time
                }
            }
            _ => UNTRUSTED,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Seen {
    size: u64,
    changed: Option<u64>,
}

impl Seen {
    pub(crate) fn of(metadata: &Metadata) -> Option<Self> {
        metadata.is_file().then(|| Self {
            size: metadata.len(),
            changed: changed(metadata),
        })
    }

    // constraint: only macOS lists stats, and there the stamp is the change time a listed stat carries
    pub(crate) fn listed(stat: ListedStat) -> Self {
        Self {
            size: stat.size,
            changed: since_epoch(stat.ctime, stat.ctime_nsec),
        }
    }
}

pub(crate) fn unchanged(stamp: Stamp, size: Option<u64>, seen: Seen, visit: bool) -> bool {
    let time = stamp & !VISITED;
    let handed_over_if_wanted = !visit || stamp & VISITED != 0;

    time != UNTRUSTED
        && handed_over_if_wanted
        && size == Some(seen.size)
        && seen.changed == Some(time)
}

// constraint: the stamp is the inode change time where there is one, because a tool that keeps modification times
// (`cp -p`, `rsync -t`, `tar x`) sets those back on a write, and nothing sets a change time back
#[cfg(unix)]
fn changed(metadata: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;

    since_epoch(metadata.ctime(), metadata.ctime_nsec())
}

fn since_epoch(seconds: i64, nanos: i64) -> Option<u64> {
    let seconds = u64::try_from(seconds).ok()?;
    let nanos = u64::try_from(nanos).ok()?;
    seconds.checked_mul(1_000_000_000)?.checked_add(nanos)
}

#[cfg(not(unix))]
fn changed(metadata: &Metadata) -> Option<u64> {
    metadata.modified().ok().and_then(nanos)
}

fn nanos(time: SystemTime) -> Option<u64> {
    u64::try_from(time.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
}
