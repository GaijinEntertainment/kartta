use std::any::Any;

use kartta_lang::path::{base, dir, forward_slashes, join, sole};
use kartta_lang::{Extraction, ImportRef, IncludeRef, RawExtraction, Repository, Resolver};

use crate::mounts::MountTable;
use crate::raw_extraction;
use crate::walk::INCLUDE_FORM;

// shortcut: the mount table is read only from the files of these names -- the settings file of a game, the
// `application.blk` of a tool and the `--config` files of the AOT compilers -- though an application may name any
// file; compile it from the extraction of every BLK file once a resolver can be built over the files it claims
const MOUNT_FILE_NAMES: [&str; 4] = [
    "settings.blk",
    "application.blk",
    "aot_config.blk",
    "libs_aot_config.blk",
];

#[derive(PartialEq)]
pub(crate) struct BlkResolver {
    mounts: MountTable,
}

impl BlkResolver {
    pub(crate) fn new(repository: &dyn Repository) -> Self {
        let mut mounts = MountTable::default();
        let stating_files = MOUNT_FILE_NAMES
            .into_iter()
            .flat_map(|name| repository.files_named(name));
        for path in stating_files {
            if let Ok(source) = repository.read(&path) {
                mounts.add(&path, &raw_extraction(&source));
            }
        }

        Self { mounts }
    }

    // constraint: the file layer reads `\` as `/`; a `#` path is relative to a root the application sets, and an
    // absolute path, a drive-letter one included, or one relative to the working directory (`:/`) names no
    // repository file
    fn target(&self, read: &str, including: &str, repository: &dyn Repository) -> Option<String> {
        if read.starts_with('%') {
            return self.mounts.resolve(read, including, repository);
        }
        let read = forward_slashes(read);
        if let Some(root_relative) = read.strip_prefix('#') {
            return Self::sole_candidate(root_relative, repository);
        }
        if read.starts_with('/') || read.contains(':') {
            return None;
        }

        indexed(join(dir(including), &read)?, repository)
    }

    fn sole_candidate(root_relative: &str, repository: &dyn Repository) -> Option<String> {
        let suffix = join(".", root_relative).filter(|suffix| !suffix.is_empty())?;
        let candidates = repository.files_named(base(&suffix)).filter(|candidate| {
            candidate
                .strip_suffix(suffix.as_str())
                .is_some_and(|root| root.is_empty() || root.ends_with('/'))
        });
        sole(candidates)
    }
}

impl Resolver for BlkResolver {
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        for directive in directives {
            if directive.form != INCLUDE_FORM {
                continue;
            }
            let read = directive.names.first().unwrap_or(&directive.spec);
            match self.target(read, path, repository) {
                Some(target) => extraction.includes.push(IncludeRef::new(target)),
                None => extraction.imports.push(ImportRef::new(
                    directive.spec,
                    Vec::new(),
                    directive.line,
                )),
            }
        }

        extraction
    }

    fn resolves_as(&self, kept: &dyn Resolver) -> bool {
        (kept as &dyn Any).downcast_ref::<Self>() == Some(self)
    }
}

// constraint: a game reads its shipped files from a vromfs, whose lookup lowercases the path over names the packer
// stores lowercased, so a target that names no file loads the one file that differs from it only in letter case
pub(crate) fn indexed(path: String, repository: &dyn Repository) -> Option<String> {
    if repository.contains(&path) {
        return Some(path);
    }
    sole(repository.files_ignoring_case(&path))
}
