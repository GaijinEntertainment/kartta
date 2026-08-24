//! The name-based half of binary classification: the extensions that decide without opening anything.

/// Every extension that settles a file as binary, grouped by where the format comes from.
///
/// The groups are what a reader extends, so each one is named rather than labelled — adding a format means finding the
/// list it belongs to, and the names are what makes that a search rather than a scan.
///
/// The whole list is one-sided on purpose. It says "binary, do not open" and never says text: an extension it does not
/// name settles nothing, and the file is read instead. A list that claimed text would have to be right about every
/// tree in the world, and being wrong would mean calling a file unsearchable that is not.
const GROUPS: &[&[&str]] = &[
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

/// `svg` is the one deliberate exception to "binary by construction": it is XML, and in practice it is machine-emitted
/// path data whose lines are of no use to a reader.
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

/// Whether the name at the end of `path` carries an extension that settles the file as binary.
///
/// `false` means the name settled nothing, never that the file is text.
pub(crate) fn named_binary(path: &str) -> bool {
    let Some(extension) = extension(path) else {
        return false;
    };

    GROUPS
        .iter()
        .flat_map(|group| group.iter())
        .any(|known| known.eq_ignore_ascii_case(extension))
}

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
    fn the_groups_settle_binary_names_and_nothing_else() {
        assert!(named_binary("assets/logo.png"));
        assert!(
            named_binary("assets/LOGO.PNG"),
            "how the name is cased is not the question"
        );
        assert!(
            !named_binary("src/main.rs"),
            "an extension no group names settles nothing"
        );
        assert!(!named_binary("README"), "and neither does having none");
    }

    #[test]
    fn no_extension_is_listed_twice() {
        let mut all: Vec<&str> = GROUPS
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
        for extension in GROUPS.iter().flat_map(|group| group.iter()) {
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
