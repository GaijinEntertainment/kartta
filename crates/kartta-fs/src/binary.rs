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
/// [`BinaryExtensions::builtin`] is the [`Default`]: images, audio, video, geometry, build artifacts, archives,
/// fonts, office documents, disk images, model weights, game engine assets, compiled shaders, Unreal packages, sound
/// banks and Dagor Engine assets. A caller adds its own extensions to it with [`union`](Self::union), or states a whole
/// set of its own with [`BinaryExtensions::new`]. An extension no file in a tree carries is not an error: most of the
/// built-in set is absent from any one tree.
///
/// Extensions compare without regard to ASCII case, and only the last extension of a name is compared — `gz` settles
/// `archive.tar.gz`. A name whose only dot leads it, such as `.gitignore`, carries no extension.
///
/// # Examples
///
/// ```
/// use kartta_fs::BinaryExtensions;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let binary = BinaryExtensions::builtin().union(&BinaryExtensions::new(["vromfs", ".PACKED"])?);
///
/// assert!(binary.matches("content/pack.vromfs"));
/// assert!(binary.matches("textures/hull.packed"));
/// assert!(binary.matches("models/tank.dag"));
/// assert!(!binary.matches("src/main.rs"));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BinaryExtensions {
    extensions: Vec<Cow<'static, str>>,
}

impl BinaryExtensions {
    /// The built-in set, every format the [type docs](Self) list.
    #[must_use]
    pub fn builtin() -> Self {
        let extensions = BUILTIN
            .iter()
            .flat_map(|group| group.iter())
            .map(|&extension| Cow::Borrowed(extension))
            .collect();

        Self::normalized(extensions)
    }

    /// A set of the caller's own extensions.
    ///
    /// A given extension may carry one leading dot and any ASCII case, so `vromfs`, `.vromfs` and `.VROMFS` are one
    /// extension.
    ///
    /// # Errors
    ///
    /// Returns [`BinaryExtensionsError`] for the first given extension that no extension of a file name can equal,
    /// naming it as it was given.
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

    /// Whether the name at the end of `path` carries an extension in the set.
    ///
    /// `false` means the name settled nothing, never that the file is text.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        extension(path).is_some_and(|extension| {
            self.extensions
                .binary_search_by(|known| {
                    known
                        .bytes()
                        .cmp(extension.bytes().map(|byte| byte.to_ascii_lowercase()))
                })
                .is_ok()
        })
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

/// Why a given extension cannot join a [`BinaryExtensions`] set: no extension of any file name can equal it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BinaryExtensionsError {
    /// The extension is empty once its leading dot is dropped.
    #[error("extension {extension:?} is empty")]
    Empty {
        /// The extension as it was given.
        extension: String,
    },
    /// The extension holds a dot past its first character, and only the last extension of a name is compared.
    #[error(
        "extension {extension:?} holds a dot, and only the last extension of a name is compared"
    )]
    Dotted {
        /// The extension as it was given.
        extension: String,
    },
    /// The extension holds a `/`, which no single path segment can.
    #[error("extension {extension:?} holds a `/`, which no file name can")]
    Separator {
        /// The extension as it was given.
        extension: String,
    },
    /// The extension holds a NUL, which no file name can.
    #[error("extension {extension:?} holds a NUL, which no file name can")]
    Nul {
        /// The extension as it was given.
        extension: String,
    },
}

const BUILTIN: [&[&str]; 15] = [
    IMAGES,
    AUDIO,
    VIDEO,
    GEOMETRY,
    ARTIFACTS,
    ARCHIVES,
    FONTS,
    DOCUMENTS,
    DISK_IMAGES,
    MODEL_WEIGHTS,
    GAME_ASSETS,
    SHADERS,
    UNREAL,
    AUDIO_MIDDLEWARE,
    DAGOR,
];

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

const IMAGES: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp", "tiff", "tif", "psd", "dds", "tga",
    "exr", "ktx", "ktx2", "astc", "pvr", "basis", "avif", "heic", "heif", "jxl", "jp2", "icns",
    "cur", "xcf", "kra", "dng", "cr2", "cr3", "nef", "arw",
];

const AUDIO: &[&str] = &[
    "wav", "mp3", "ogg", "flac", "aac", "wma", "m4a", "opus", "aif", "aiff", "caf", "mid", "midi",
    "ape", "wv", "amr", "oga", "weba",
];

const VIDEO: &[&str] = &[
    "mp4", "avi", "mkv", "mov", "wmv", "webm", "flv", "m4v", "mpg", "mpeg", "ogv", "3gp", "3g2",
    "f4v", "vob", "rmvb", "asf", "m2ts", "bik", "bk2",
];

const GEOMETRY: &[&str] = &[
    "glb", "blend", "3ds", "usdc", "usdz", "3dm", "max", "mb", "c4d", "skp", "vdb", "dwg",
];

const ARTIFACTS: &[&str] = &[
    "exe", "dll", "so", "dylib", "o", "a", "wasm", "class", "jar", "pyc", "pyo", "node", "rlib",
    "rmeta", "pyd", "beam", "dex", "gch", "ilk", "idb", "apk", "aab", "ipa", "whl", "nupkg", "war",
    "ear", "aar", "msi", "msix", "appx", "deb", "rpm", "appimage", "crx", "xpi",
];

const ARCHIVES: &[&str] = &[
    "zip", "tar", "gz", "bz2", "xz", "7z", "rar", "zst", "lz4", "bz", "tgz", "tbz", "tbz2", "txz",
    "tlz", "lz", "lzma", "lzo", "z", "zipx", "cab", "arj", "lzh", "lha",
];

const FONTS: &[&str] = &[
    "ttf", "otf", "woff", "woff2", "eot", "ttc", "otc", "pfb", "fon",
];

const DOCUMENTS: &[&str] = &[
    "pdf", "docx", "xls", "xlsx", "ppt", "pptx", "mdb", "odt", "ods", "odp", "odg", "epub", "docm",
    "dotx", "dotm", "xlsm", "xlsb", "xltx", "xltm", "pptm", "potx", "ppsx", "ppsm", "xps", "oxps",
    "vsdx", "mobi", "djvu", "accdb", "kdbx", "dbf",
];

const DISK_IMAGES: &[&str] = &[
    "img", "iso", "dmg", "vdi", "qcow2", "raw", "vhdx", "ova", "wim",
];

const MODEL_WEIGHTS: &[&str] = &[
    "safetensors",
    "gguf",
    "ggml",
    "onnx",
    "npy",
    "npz",
    "h5",
    "parquet",
    "arrow",
    "tflite",
    "keras",
    "mlmodel",
    "caffemodel",
    "feather",
    "avro",
    "hdf5",
    "ckpt",
];

const GAME_ASSETS: &[&str] = &[
    "bin", "pak", "bundle", "bsp", "nav", "vtf", "vpk", "unity3d", "ress", "xnb", "wad", "pk3",
];

const SHADERS: &[&str] = &["spv", "cso", "dxbc", "metallib", "dxil"];

const UNREAL: &[&str] = &["uasset", "umap", "ubulk", "upk", "uexp", "ucas", "utoc"];

const AUDIO_MIDDLEWARE: &[&str] = &["fsb", "fev", "bnk", "bank", "wem", "acb", "awb", "xwb"];

// constraint: `dag` also names the workflow files of the Condor job scheduler, which are text, so a tree that holds
// those states a set of its own
const DAGOR: &[&str] = &["dag", "ddsx", "grp", "a2d"];

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
    fn the_builtin_set_is_every_listed_extension_and_the_default() {
        let listed = BinaryExtensions::new(BUILTIN.iter().flat_map(|group| group.iter()))
            .expect("every listed extension is one a caller could give");

        assert_eq!(BinaryExtensions::builtin(), listed);
        assert_eq!(BinaryExtensions::default(), listed);
    }

    #[test]
    fn a_given_extension_drops_one_leading_dot_and_its_case() {
        let given = BinaryExtensions::new(["vromfs", ".VROMFS", ".Vromfs", "nosuchformat"])
            .expect("every given string is an extension");

        assert_eq!(
            given.extensions,
            ["nosuchformat", "vromfs"],
            "one extension however it is spelled, and an extension no tree holds is still one"
        );
    }

    #[test]
    fn an_extension_no_file_name_can_carry_is_refused_by_name() {
        let refused =
            |given: &str| BinaryExtensions::new([given]).expect_err("the extension is refused");

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
    fn the_first_refused_extension_is_the_one_named() {
        assert_eq!(
            BinaryExtensions::new(["vromfs", "a/b", ""]),
            Err(BinaryExtensionsError::Separator {
                extension: "a/b".to_owned()
            })
        );
    }

    #[test]
    fn union_adds_to_a_set() {
        let binary = BinaryExtensions::builtin()
            .union(&BinaryExtensions::new(["vromfs"]).expect("an extension"));

        assert!(binary.matches("pack.vromfs"));
        assert!(binary.matches("logo.png"), "the built-in set stays");
    }

    #[test]
    fn a_set_holds_each_extension_once_in_byte_order() {
        let builtin = BinaryExtensions::builtin();
        let twice = builtin.clone().union(&builtin);

        assert_eq!(twice, builtin);

        let mut ordered = twice.extensions.clone();
        ordered.sort_unstable();
        assert_eq!(twice.extensions, ordered);
    }

    #[test]
    fn no_builtin_extension_names_a_text_format() {
        let builtin = BinaryExtensions::builtin();
        let text = [
            "gltf", "dae", "usda", "usd", "obj", "stl", "fbx", "asset", "lib", "pdb", "hdr",
            "vmdk", "db", "dat", "res", "mdl", "pck", "pt", "pth", "doc", "sqlite",
        ];

        for extension in text {
            assert!(
                !builtin.matches(&format!("file.{extension}")),
                "{extension} names a text format, or shares its name with one"
            );
        }
    }

    #[test]
    fn no_extension_is_listed_twice() {
        let mut all: Vec<&str> = BUILTIN
            .iter()
            .flat_map(|group| group.iter().copied())
            .collect();
        all.sort_unstable();

        let mut unique = all.clone();
        unique.dedup();

        assert_eq!(all, unique, "a duplicate means two groups claim one format");
    }

    #[test]
    fn every_listed_extension_is_lowercase_and_bare() {
        for extension in BUILTIN.iter().flat_map(|group| group.iter()) {
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
