use std::num::NonZeroU32;

/// A consumer of file content, handed each wanted text file once, during the walk that indexes it.
///
/// A build that is given one reads every regular file the visitor [`wants`](ContentVisitor::wants) while the walk is
/// in that file's directory, and hands its whole content to [`visit`](ContentVisitor::visit). That is the one read of
/// the file a build makes: a consumer that parses, hashes or indexes content does its work there rather than opening
/// the file again after the walk, when the filesystem's caches of the tree have long moved on.
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
}

/// Names one file a build read, from the moment it was read.
///
/// A position does not exist until the walk is over, because positions follow the order of the finished tree, so a
/// file read during the walk is named by this instead, and
/// [`Index::content_position`](crate::Index::content_position) maps it to its position afterwards. The id belongs to
/// one build: two builds of one tree may number the same file differently, because the walk's threads read files in
/// whatever order they reach them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentId(pub(crate) NonZeroU32);

impl ContentId {
    pub(crate) const fn slot(self) -> usize {
        self.0.get() as usize - 1
    }
}
