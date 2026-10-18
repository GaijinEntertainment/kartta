use std::num::NonZeroU32;

use kartta_cache::{ContentHash, Namespace};

use crate::cache::SharedCache;

/// A consumer of file content, handed each wanted text file once, during the walk that indexes it.
///
/// A build that is given one reads every regular file the visitor [`wants`](ContentVisitor::wants) while the walk is
/// in that file's directory, and hands its whole content to [`visit`](ContentVisitor::visit). That is the one read of
/// the file a build makes: a consumer that parses, hashes or indexes content does its work there rather than opening
/// the file again after the walk, when the filesystem's caches of the tree have long moved on.
///
/// A build given [`BuildOptions::content_cache`](crate::BuildOptions::content_cache) lends the visitor the same
/// store, keyed by each file's content: what the visitor computes per file it can take from [`Content::cached`] and
/// share through [`Content::cache`], so builds of nearly identical trees compute it once.
///
/// Both methods run on the walk's threads, many at a time and in no particular order, which is why the visitor is
/// [`Sync`]. [`Content::id`] names each file for the rest of the build, and
/// [`Index::content_position`](crate::Index::content_position) turns it into the file's position once the build is
/// done — which is where a consumer that needs index order gets it.
///
/// Only text is visited. A file whose extension is in
/// [`BuildOptions::binary_extensions`](crate::BuildOptions::binary_extensions) is never opened, and a file holding a
/// NUL in its first 64 KiB is read that far and no further, by the rule [`Index::is_binary`](crate::Index::is_binary)
/// applies. A file that cannot be read is reported in [`BuildOutcome::errors`](crate::BuildOutcome::errors). A panic in
/// either method ends the build with that panic.
///
/// # Examples
///
/// ```
/// use std::sync::atomic::{AtomicUsize, Ordering};
///
/// use kartta_fs::{BuildOptions, Content, ContentVisitor, Index};
///
/// struct RustBytes(AtomicUsize);
///
/// impl ContentVisitor for RustBytes {
///     fn wants(&self, path: &str) -> bool {
///         path.ends_with(".rs")
///     }
///
///     fn visit(&self, content: Content<'_>) {
///         self.0.fetch_add(content.bytes().len(), Ordering::Relaxed);
///     }
/// }
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// std::fs::write(dir.path().join("main.rs"), "fn main() {}")?;
/// std::fs::write(dir.path().join("notes.md"), "unread")?;
///
/// let counted = RustBytes(AtomicUsize::new(0));
/// let outcome = Index::build_with(dir.path(), &BuildOptions::default(), Some(&counted), None)?;
/// assert!(outcome.errors().is_empty());
/// assert_eq!(counted.0.into_inner(), 12);
/// # Ok(())
/// # }
/// ```
pub trait ContentVisitor: Sync {
    /// Whether to read the regular file at `path`, a root-relative forward-slash path as the index spells it.
    ///
    /// Asked once per file the ignore rules kept. A file it turns down is not read for the visitor at all — with one
    /// exception the build cannot skip: a `.gitignore` the ignore rules need is read for those rules whatever this
    /// answers, and when this wants it, the visitor is handed that same read rather than a second one.
    fn wants(&self, path: &str) -> bool;

    /// One wanted text file's whole content.
    fn visit(&self, content: Content<'_>);
}

/// One text file's content, as a [`ContentVisitor`] is handed it.
///
/// The bytes are the file's as the build read them, borrowed for the length of the call; a visitor that keeps them
/// copies them.
#[derive(Debug, Clone, Copy)]
pub struct Content<'a> {
    pub(crate) id: ContentId,
    pub(crate) path: &'a str,
    pub(crate) bytes: &'a [u8],
    pub(crate) cache: Option<(&'a SharedCache, ContentHash)>,
}

impl<'a> Content<'a> {
    /// The name this file goes by for the rest of the build.
    #[must_use]
    pub const fn id(&self) -> ContentId {
        self.id
    }

    /// The file's root-relative forward-slash path.
    #[must_use]
    pub const fn path(&self) -> &'a str {
        self.path
    }

    /// The file's content.
    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// What a build sharing the cache stored under `namespace` for these bytes, borrowed from the store for the length
    /// of the call, or `None` when the store holds nothing there or the build was given no cache.
    ///
    /// The store answers with what it held when the build opened it, so an entry this build caches is found by the next
    /// build, not by this one. The namespace names the producer and its version — see [`Namespace::new`] — so data a
    /// different version computed is never answered.
    ///
    /// # Examples
    ///
    /// ```
    /// use kartta_fs::{BuildOptions, Content, ContentVisitor, Index, Namespace};
    ///
    /// struct Lines(Namespace);
    ///
    /// impl ContentVisitor for Lines {
    ///     fn wants(&self, path: &str) -> bool {
    ///         path.ends_with(".rs")
    ///     }
    ///
    ///     fn visit(&self, content: Content<'_>) {
    ///         if content.cached(&self.0).is_none() {
    ///             let lines = content.bytes().iter().filter(|&&byte| byte == b'\n').count();
    ///             content.cache(&self.0, &lines.to_le_bytes());
    ///         }
    ///     }
    /// }
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let tree = tempfile::tempdir()?;
    /// std::fs::write(tree.path().join("main.rs"), "fn main() {}\n")?;
    /// let cache = tempfile::tempdir()?;
    /// let mut options = BuildOptions::default();
    /// options.content_cache = Some(cache.path().to_path_buf());
    ///
    /// let lines = Lines(Namespace::new("lines", b"newline count, little-endian usize, 1"));
    /// let outcome = Index::build_with(tree.path(), &options, Some(&lines), None)?;
    /// assert!(outcome.errors().is_empty());
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn cached(&self, namespace: &Namespace) -> Option<&'a [u8]> {
        let (cache, hash) = self.cache?;
        cache.get(namespace, &hash)
    }

    /// Shares `payload` under `namespace` for these bytes with every build that uses the same cache, or does nothing
    /// when the build was given none.
    ///
    /// The payload joins the pack the build publishes once its walk is done, beside the content index's trigrams, and
    /// a payload already given for the same namespace and bytes in this build is kept rather than replaced. A failure
    /// to write it costs the sharing only, and is reported as [`BuildError::Cache`](crate::BuildError::Cache).
    pub fn cache(&self, namespace: &Namespace, payload: &[u8]) {
        if let Some((cache, hash)) = self.cache {
            cache.insert(namespace, &hash, payload);
        }
    }
}

/// Names one file a build read, from the moment it was read.
///
/// A position does not exist until the walk is over, because positions follow the order of the finished tree, so a
/// file read during the walk is named by this instead, and
/// [`Index::content_position`](crate::Index::content_position) maps it to its position afterwards. The id belongs to
/// one index: two builds of one tree may number the same file differently, because the walk's threads read files in
/// whatever order they reach them, and a [`refresh`](crate::Index::refresh) numbers its reads past every id the index
/// handed out before, so within one index an id names one file only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId(pub(crate) NonZeroU32);

impl ContentId {
    pub(crate) const fn slot(self) -> usize {
        self.0.get() as usize - 1
    }

    pub(crate) const fn from_slot(slot: u32) -> Self {
        Self(NonZeroU32::MIN.saturating_add(slot))
    }
}
