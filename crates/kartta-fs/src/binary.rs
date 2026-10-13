//! [`BinaryExtensions`] — the name-based half of binary classification: the extensions that decide without opening
//! anything.

use std::borrow::Cow;

/// The file extensions that settle a file as binary without the file being read.
///
/// The set is one-sided on purpose. It says "binary, do not open" and never says text: an extension it does not hold
/// settles nothing, and the file is read instead. Both mistakes it can make have a cost, and they are not equal.
///
/// **Holding an extension wrongly is the expensive one.** A held extension is answered without a probe, so a text
/// format in the set is reported binary and a content search passes over it — a file nobody can find. Leaving a binary
/// format out costs one read, which finds a NUL and answers correctly anyway. So a format earns its place by being
/// binary *by construction*, where no instance of it can be text, and never by being merely generated or large.
///
/// That asymmetry is also the argument against a companion set of text extensions: it would have to make the same
/// unconditional claim about formats that cannot support it, and `json`, `txt` and `xml` all routinely carry an
/// embedded blob.
///
/// # Composing a set
///
/// [`BinaryExtensions::builtin`] is every [`BinaryGroup`] and is the [`Default`]. A set is built from the built-in
/// groups and the caller's own extensions with [`union`](Self::union) and [`difference`](Self::difference), so taking
/// a whole group out and taking one extension out are the same operation. An extension no file in a tree carries is
/// not an error: most of the built-in set is absent from any one tree.
///
/// Extensions compare without regard to ASCII case, and only the last extension of a name is compared — `gz` settles
/// `archive.tar.gz`. A name whose only dot leads it, such as `.gitignore`, carries no extension.
///
/// # Examples
///
/// ```
/// use kartta_fs::{BinaryExtensions, BinaryGroup};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let binary = BinaryExtensions::builtin()
///     .difference(&BinaryExtensions::group(BinaryGroup::Dagor))
///     .union(&BinaryExtensions::new(["vromfs", ".DDSX"])?);
///
/// assert!(binary.matches("content/pack.vromfs"));
/// assert!(binary.matches("textures/hull.ddsx"));
/// assert!(!binary.matches("models/tank.dag"));
/// assert!(!binary.matches("src/main.rs"));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BinaryExtensions {
    extensions: Vec<Cow<'static, str>>,
}

impl BinaryExtensions {
    /// Every built-in [`BinaryGroup`].
    #[must_use]
    pub fn builtin() -> Self {
        Self::of_groups(BinaryGroup::ALL)
    }

    /// The extensions of one built-in group.
    #[must_use]
    pub fn group(group: BinaryGroup) -> Self {
        Self::of_groups(&[group])
    }

    /// A set of the caller's own extensions.
    ///
    /// An entry may carry one leading dot and any ASCII case, so `vromfs`, `.vromfs` and `.VROMFS` are one extension.
    ///
    /// # Errors
    ///
    /// Returns [`BinaryExtensionsError`] for the first entry that no extension of a file name can equal, naming that
    /// entry as it was given.
    pub fn new<I, E>(extensions: I) -> Result<Self, BinaryExtensionsError>
    where
        I: IntoIterator<Item = E>,
        E: AsRef<str>,
    {
        let parsed = extensions
            .into_iter()
            .map(|extension| parse(extension.as_ref()).map(Cow::Owned))
            .collect::<Result<_, _>>()?;

        Ok(Self::normalized(parsed))
    }

    /// This set with every extension of `other` added.
    #[must_use]
    pub fn union(mut self, other: &Self) -> Self {
        self.extensions.extend(other.extensions.iter().cloned());

        Self::normalized(self.extensions)
    }

    /// This set with every extension of `other` taken out.
    #[must_use]
    pub fn difference(mut self, other: &Self) -> Self {
        self.extensions
            .retain(|extension| other.extensions.binary_search(extension).is_err());

        self
    }

    /// The extensions in the set, lowercase and in byte order.
    pub fn extensions(&self) -> impl ExactSizeIterator<Item = &str> {
        self.extensions.iter().map(AsRef::as_ref)
    }

    /// Whether the name at the end of `path` carries an extension in the set.
    ///
    /// `false` means the name settled nothing, never that the file is text.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        let Some(extension) = extension(path) else {
            return false;
        };

        self.extensions
            .binary_search_by(|known| {
                known
                    .bytes()
                    .cmp(extension.bytes().map(|byte| byte.to_ascii_lowercase()))
            })
            .is_ok()
    }

    fn of_groups(groups: &[BinaryGroup]) -> Self {
        let extensions = groups
            .iter()
            .flat_map(|group| group.extensions())
            .map(|&extension| Cow::Borrowed(extension))
            .collect();

        Self::normalized(extensions)
    }

    fn normalized(mut extensions: Vec<Cow<'static, str>>) -> Self {
        extensions.sort_unstable();
        extensions.dedup();

        Self { extensions }
    }
}

impl Default for BinaryExtensions {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Why an entry cannot join a [`BinaryExtensions`] set: no extension of any file name can equal it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BinaryExtensionsError {
    /// The entry is empty once its leading dot is dropped.
    #[error("extension {extension:?} is empty")]
    Empty {
        /// The entry as it was given.
        extension: String,
    },
    /// The entry holds a dot past its first character, and only the last extension of a name is compared.
    #[error(
        "extension {extension:?} holds a dot, and only the last extension of a name is compared"
    )]
    Dotted {
        /// The entry as it was given.
        extension: String,
    },
    /// The entry holds a `/`, which no single path segment can.
    #[error("extension {extension:?} holds a `/`, which no file name can")]
    Separator {
        /// The entry as it was given.
        extension: String,
    },
    /// The entry holds a NUL, which no file name can.
    #[error("extension {extension:?} holds a NUL, which no file name can")]
    Nul {
        /// The entry as it was given.
        extension: String,
    },
}

/// A built-in group of binary extensions, named for where its formats come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum BinaryGroup {
    /// Raster images, texture containers, and `svg`.
    Images,
    /// Encoded audio.
    Audio,
    /// Encoded video.
    Video,
    /// 3D scene and mesh formats.
    Geometry,
    /// What a compiler or linker emits.
    Artifacts,
    /// Compressed archives.
    Archives,
    /// Font files.
    Fonts,
    /// Office documents and database files.
    Documents,
    /// Disk and virtual machine images.
    DiskImages,
    /// Machine learning weights and columnar data.
    ModelWeights,
    /// Generic game engine asset containers.
    GameAssets,
    /// Compiled GPU shaders.
    Shaders,
    /// Unreal Engine packages.
    Unreal,
    /// FMOD and Wwise sound banks.
    AudioMiddleware,
    /// Dagor Engine assets.
    Dagor,
}

impl BinaryGroup {
    /// Every built-in group.
    pub const ALL: &[Self] = &[
        Self::Images,
        Self::Audio,
        Self::Video,
        Self::Geometry,
        Self::Artifacts,
        Self::Archives,
        Self::Fonts,
        Self::Documents,
        Self::DiskImages,
        Self::ModelWeights,
        Self::GameAssets,
        Self::Shaders,
        Self::Unreal,
        Self::AudioMiddleware,
        Self::Dagor,
    ];

    const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Images => IMAGES,
            Self::Audio => AUDIO,
            Self::Video => VIDEO,
            Self::Geometry => GEOMETRY,
            Self::Artifacts => ARTIFACTS,
            Self::Archives => ARCHIVES,
            Self::Fonts => FONTS,
            Self::Documents => DOCUMENTS,
            Self::DiskImages => DISK_IMAGES,
            Self::ModelWeights => MODEL_WEIGHTS,
            Self::GameAssets => GAME_ASSETS,
            Self::Shaders => SHADERS,
            Self::Unreal => UNREAL,
            Self::AudioMiddleware => AUDIO_MIDDLEWARE,
            Self::Dagor => DAGOR,
        }
    }
}

/// The extension `given` names, lowercase and without its leading dot.
fn parse(given: &str) -> Result<String, BinaryExtensionsError> {
    let bare = given.strip_prefix('.').unwrap_or(given);
    let extension = || given.to_owned();

    if bare.is_empty() {
        Err(BinaryExtensionsError::Empty {
            extension: extension(),
        })
    } else if bare.contains('.') {
        Err(BinaryExtensionsError::Dotted {
            extension: extension(),
        })
    } else if bare.contains('/') {
        Err(BinaryExtensionsError::Separator {
            extension: extension(),
        })
    } else if bare.contains('\0') {
        Err(BinaryExtensionsError::Nul {
            extension: extension(),
        })
    } else {
        Ok(bare.to_ascii_lowercase())
    }
}

/// `svg` is the one deliberate exception to "binary by construction": it is XML, so listing it does make a text format
/// unsearchable here. It earns that because in practice it is machine-emitted path data whose lines are of no use to a
/// reader — the expensive mistake made knowingly, rather than a case the rule missed.
const IMAGES: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp", "tiff", "tif", "psd", "dds", "tga",
    "hdr", "exr", "ktx", "ktx2", "astc", "pvr", "basis",
];

const AUDIO: &[&str] = &["wav", "mp3", "ogg", "flac", "aac", "wma", "m4a", "opus"];

const VIDEO: &[&str] = &["mp4", "avi", "mkv", "mov", "wmv", "webm", "flv"];

const GEOMETRY: &[&str] = &[
    "fbx", "obj", "gltf", "glb", "blend", "dae", "3ds", "stl", "usd", "usda", "usdc", "usdz",
];

/// What a compiler or linker emits.
const ARTIFACTS: &[&str] = &[
    "exe", "dll", "so", "dylib", "o", "a", "lib", "pdb", "wasm", "class", "jar", "pyc", "pyo",
    "node",
];

const ARCHIVES: &[&str] = &["zip", "tar", "gz", "bz2", "xz", "7z", "rar", "zst", "lz4"];

const FONTS: &[&str] = &["ttf", "otf", "woff", "woff2", "eot"];

const DOCUMENTS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "sqlite", "db", "mdb",
];

const DISK_IMAGES: &[&str] = &["img", "iso", "dmg", "vmdk", "vdi", "qcow2", "raw"];

const MODEL_WEIGHTS: &[&str] = &[
    "safetensors",
    "gguf",
    "ggml",
    "onnx",
    "pt",
    "pth",
    "npy",
    "npz",
    "h5",
    "parquet",
    "arrow",
];

const GAME_ASSETS: &[&str] = &[
    "bin", "pak", "dat", "res", "asset", "bundle", "bank", "bsp", "nav", "lightmap", "cubemap",
    "mdl", "vtf", "vpk", "pck",
];

const SHADERS: &[&str] = &["spv", "cso", "dxbc", "metallib"];

const UNREAL: &[&str] = &["uasset", "umap", "ubulk", "upk"];

const AUDIO_MIDDLEWARE: &[&str] = &["fsb", "fev", "bnk"];

const DAGOR: &[&str] = &["dag", "dynmodel", "rendinst", "composit", "gameobj", "lod"];

/// The extension of the last segment of `path`, or `None` when it has none.
///
/// A leading dot names a hidden file rather than an extension, so `.gitignore` has none — which is what keeps a
/// dotfile off a list its bare suffix would otherwise land it on.
fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    let (stem, extension) = name.rsplit_once('.')?;

    (!stem.is_empty()).then_some(extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extension_needs_a_stem_before_it() {
        assert_eq!(extension("src/main.rs"), Some("rs"));
        assert_eq!(extension("Makefile"), None);
        assert_eq!(
            extension("a/.gitignore"),
            None,
            "a dotfile carries a name, not an extension"
        );
        assert_eq!(
            extension("a.b/plain"),
            None,
            "the dot belongs to the directory"
        );
        assert_eq!(extension("archive.tar.gz"), Some("gz"), "the last one wins");
    }

    #[test]
    fn the_builtin_set_settles_binary_names_and_nothing_else() {
        let builtin = BinaryExtensions::builtin();

        assert!(builtin.matches("assets/logo.png"));
        assert!(
            builtin.matches("assets/LOGO.PNG"),
            "how the name is cased is not the question"
        );
        assert!(
            !builtin.matches("src/main.rs"),
            "an extension no group names settles nothing"
        );
        assert!(!builtin.matches("README"), "and neither does having none");
    }

    #[test]
    fn the_builtin_set_is_every_group_and_the_default() {
        let every_group = BinaryGroup::ALL.iter().fold(
            BinaryExtensions::new([""; 0]).expect("an empty set"),
            |set, &group| set.union(&BinaryExtensions::group(group)),
        );

        assert_eq!(BinaryExtensions::builtin(), every_group);
        assert_eq!(BinaryExtensions::default(), every_group);
    }

    #[test]
    fn all_holds_every_group_once() {
        let every = [
            BinaryGroup::Images,
            BinaryGroup::Audio,
            BinaryGroup::Video,
            BinaryGroup::Geometry,
            BinaryGroup::Artifacts,
            BinaryGroup::Archives,
            BinaryGroup::Fonts,
            BinaryGroup::Documents,
            BinaryGroup::DiskImages,
            BinaryGroup::ModelWeights,
            BinaryGroup::GameAssets,
            BinaryGroup::Shaders,
            BinaryGroup::Unreal,
            BinaryGroup::AudioMiddleware,
            BinaryGroup::Dagor,
        ];

        for group in every {
            match group {
                BinaryGroup::Images
                | BinaryGroup::Audio
                | BinaryGroup::Video
                | BinaryGroup::Geometry
                | BinaryGroup::Artifacts
                | BinaryGroup::Archives
                | BinaryGroup::Fonts
                | BinaryGroup::Documents
                | BinaryGroup::DiskImages
                | BinaryGroup::ModelWeights
                | BinaryGroup::GameAssets
                | BinaryGroup::Shaders
                | BinaryGroup::Unreal
                | BinaryGroup::AudioMiddleware
                | BinaryGroup::Dagor => assert!(
                    BinaryGroup::ALL.contains(&group),
                    "{group:?} is missing from ALL"
                ),
            }
        }
        assert_eq!(
            BinaryGroup::ALL.len(),
            every.len(),
            "ALL lists a group twice"
        );
    }

    #[test]
    fn a_given_extension_drops_one_leading_dot_and_its_case() {
        let given = BinaryExtensions::new(["vromfs", ".VROMFS", ".Vromfs", "nosuchformat"])
            .expect("every entry is an extension");

        assert_eq!(
            given.extensions().collect::<Vec<_>>(),
            ["nosuchformat", "vromfs"],
            "one extension however it is spelled, and an extension no tree holds is still one"
        );
    }

    #[test]
    fn an_entry_no_file_name_can_carry_is_refused_by_name() {
        let refused =
            |entry: &str| BinaryExtensions::new([entry]).expect_err("the entry is refused");

        assert_eq!(
            refused(""),
            BinaryExtensionsError::Empty {
                extension: String::new()
            }
        );
        assert_eq!(
            refused("."),
            BinaryExtensionsError::Empty {
                extension: ".".to_owned()
            }
        );
        assert_eq!(
            refused("tar.gz"),
            BinaryExtensionsError::Dotted {
                extension: "tar.gz".to_owned()
            }
        );
        assert_eq!(
            refused("..gz"),
            BinaryExtensionsError::Dotted {
                extension: "..gz".to_owned()
            },
            "only one leading dot is dropped"
        );
        assert_eq!(
            refused("a/b"),
            BinaryExtensionsError::Separator {
                extension: "a/b".to_owned()
            }
        );
        assert_eq!(
            refused("a\0b"),
            BinaryExtensionsError::Nul {
                extension: "a\0b".to_owned()
            }
        );
    }

    #[test]
    fn the_first_refused_entry_is_the_one_named() {
        assert_eq!(
            BinaryExtensions::new(["vromfs", "a/b", ""]),
            Err(BinaryExtensionsError::Separator {
                extension: "a/b".to_owned()
            })
        );
    }

    #[test]
    fn union_adds_and_difference_takes_out() {
        let custom = BinaryExtensions::new(["vromfs"]).expect("an extension");
        let binary = BinaryExtensions::builtin()
            .difference(&BinaryExtensions::group(BinaryGroup::Dagor))
            .difference(&BinaryExtensions::new(["svg"]).expect("an extension"))
            .union(&custom);

        assert!(binary.matches("pack.vromfs"));
        assert!(binary.matches("logo.png"), "the rest of a group stays");
        assert!(!binary.matches("logo.svg"), "one extension came out");
        assert!(!binary.matches("tank.dag"), "and so did a whole group");
    }

    #[test]
    fn a_set_holds_each_extension_once_in_byte_order() {
        let images = BinaryExtensions::group(BinaryGroup::Images);
        let twice = images.clone().union(&images);

        assert_eq!(twice, images);

        let extensions: Vec<&str> = twice.extensions().collect();
        let mut ordered = extensions.clone();
        ordered.sort_unstable();
        assert_eq!(extensions, ordered);
    }

    #[test]
    fn no_extension_is_listed_twice() {
        let mut all: Vec<&str> = BinaryGroup::ALL
            .iter()
            .flat_map(|group| group.extensions().iter().copied())
            .collect();
        all.sort_unstable();

        let mut unique = all.clone();
        unique.dedup();

        assert_eq!(all, unique, "a duplicate means two groups claim one format");
    }

    #[test]
    fn every_listed_extension_is_lowercase_and_bare() {
        for extension in BinaryGroup::ALL.iter().flat_map(|group| group.extensions()) {
            assert_eq!(
                *extension,
                extension.to_ascii_lowercase(),
                "the comparison folds case, so a listed spelling that is not lowercase is dead weight"
            );
            assert!(
                !extension.starts_with('.'),
                "{extension} carries a dot the parsed extension never has"
            );
        }
    }
}
