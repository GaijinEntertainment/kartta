use kartta_lang::path::{base, dir, join};
use kartta_lang::{
    Extraction, ImportRef, IncludeRef, RawExtraction, Repository, Resolver, Symbol, SymbolFlags,
    SymbolKind,
};

use crate::{EXTENSIONS, QUOTED_INCLUDE};

const HEADER_EXTENSIONS: [&str; 6] = ["", ".h", ".hpp", ".c", ".cpp", ".cc"];

pub(crate) struct Includes<'r> {
    repository: &'r dyn Repository,
}

impl<'r> Includes<'r> {
    pub(crate) const fn new(repository: &'r dyn Repository) -> Self {
        Self { repository }
    }

    fn include(&self, spec: &str, quoted: bool, path: &str) -> Option<String> {
        if spec.starts_with("./") || spec.starts_with("../") {
            return self.probe(&join(dir(path), spec)?);
        }

        if quoted
            && let Some(local) = join(dir(path), spec)
            && self.repository.contains(&local)
        {
            return Some(local);
        }

        if self.repository.contains(spec) {
            return Some(spec.to_owned());
        }
        let mut candidates: Vec<String> = self.repository.files_named(base(spec)).collect();
        let mut suffixed = candidates
            .iter()
            .filter(|candidate| ends_at_segment(candidate, spec));
        match (suffixed.next(), suffixed.next()) {
            (Some(hit), None) => return Some(hit.clone()),
            (Some(_), Some(_)) => return enclosing_copy(&candidates, spec, path),
            (None, _) => {}
        }
        if candidates.len() == 1 {
            return candidates.pop();
        }

        self.probe(spec)
    }

    fn probe(&self, stem: &str) -> Option<String> {
        HEADER_EXTENSIONS
            .iter()
            .map(|extension| format!("{stem}{extension}"))
            .find(|candidate| self.repository.contains(candidate))
    }
}

impl Resolver for Includes<'_> {
    fn resolve(&self, raw: RawExtraction, path: &str) -> Extraction {
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(is_test_file(path))
            .with_generated(name_marks_generated(base(path)));
        extraction.flag_symbols(path_flags);
        if is_c_source(path) {
            for symbol in &mut extraction.symbols {
                symbol.exported |= has_c_external_linkage(symbol);
            }
        }

        for directive in directives {
            match self.include(&directive.spec, directive.form == QUOTED_INCLUDE, path) {
                Some(target) => extraction.includes.push(IncludeRef { target }),
                None => extraction.imports.push(ImportRef {
                    module: directive.spec,
                    names: Vec::new(),
                    line: directive.line,
                }),
            }
        }

        extraction
    }
}

fn is_c_source(path: &str) -> bool {
    base(path)
        .rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("c"))
}

// constraint: C gives a file-scope `const` external linkage where C++ gives it internal linkage, and the extraction
// reads every file as C++, so only the signature still shows a `static` that keeps a C constant to its file
fn has_c_external_linkage(symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Constant
        && symbol.owner.is_none()
        && !symbol.signature.as_deref().is_some_and(|signature| {
            signature
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|word| word == "static")
        })
}

// constraint: several copies of a library each end in the specifier, and a file of one copy reaches its own through
// the include path its build sets, so the copy whose root holds the including file is meant, the deepest root if
// roots nest; a file under no copy's root names none of them, as a system header the repository vendors twice
fn enclosing_copy(candidates: &[String], spec: &str, path: &str) -> Option<String> {
    let mut roots = candidates
        .iter()
        .filter(|candidate| ends_at_segment(candidate, spec))
        .map(|candidate| (&candidate[..candidate.len() - spec.len()], candidate))
        .filter(|(root, _)| path.starts_with(root));
    let mut deepest = roots.next()?;
    let mut tied = false;
    for (root, candidate) in roots {
        if root.len() > deepest.0.len() {
            deepest = (root, candidate);
            tied = false;
        } else if root.len() == deepest.0.len() {
            tied = true;
        }
    }
    (!tied).then(|| deepest.1.clone())
}

fn ends_at_segment(path: &str, spec: &str) -> bool {
    path.strip_suffix(spec)
        .is_some_and(|head| head.ends_with('/'))
}

fn is_test_file(path: &str) -> bool {
    let name = base(path);
    let stem = name.rfind('.').map_or(name, |dot| &name[..dot]);
    if stem.ends_with("Test")
        || stem.ends_with("_test")
        || stem.ends_with("_unittest")
        || stem.starts_with("test_")
    {
        return true;
    }

    path.split('/')
        .any(|component| matches!(component, "test" | "tests" | "unittest" | "unittests"))
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "`.pb` is the stem protobuf writes, always in lower case, not an extension a person spells"
)]
fn name_marks_generated(name: &str) -> bool {
    if name.ends_with("_generated.h")
        || name.contains(".gen.")
        || name.contains("_gen.")
        || name.contains("_AOT.")
    {
        return true;
    }

    name.rsplit_once('.')
        .filter(|(_, extension)| EXTENSIONS.contains(extension))
        .map_or(name, |(stem, _)| stem)
        .ends_with(".pb")
}

#[cfg(test)]
mod tests {
    use kartta_lang::{Language, MemoryRepository};

    use super::*;
    use crate::Cpp;

    fn resolved(source: &[u8], path: &str, files: &[&'static str]) -> Extraction {
        let cpp = Cpp::new();
        let repository = MemoryRepository::of_paths(files);
        cpp.resolver(&repository).resolve(cpp.extract(source), path)
    }

    fn targets(extraction: &Extraction) -> Vec<&str> {
        extraction
            .includes
            .iter()
            .map(|include| include.target.as_str())
            .collect()
    }

    fn modules(extraction: &Extraction) -> Vec<&str> {
        extraction
            .imports
            .iter()
            .map(|import| import.module.as_str())
            .collect()
    }

    #[test]
    fn include_resolves_against_the_repository_and_a_miss_stays_an_import() {
        let extraction = resolved(
            b"#include \"core/util.h\"\n#include <vector>\nvoid anchor() {}\n",
            "src/widget.cpp",
            &["src/core/util.h", "src/widget.cpp"],
        );

        assert_eq!(extraction.file, "src/widget.cpp");
        assert_eq!(targets(&extraction), ["src/core/util.h"]);
        assert_eq!(modules(&extraction), ["vector"]);
        assert_eq!(extraction.imports[0].line, 2);
    }

    #[test]
    fn include_resolver_matches_segment_suffix_and_sole_candidate() {
        let extraction = resolved(
            b"#include \"core/widget.h\"\n#include \"other.h\"\n#include \"lib/util.h\"\nvoid anchor() {}\n",
            "src/main.cpp",
            &["src/xcore/widget.h", "src/core/widget.h", "third_party/other.h", "a/util.h", "b/util.h"],
        );

        assert_eq!(
            targets(&extraction),
            ["src/core/widget.h", "third_party/other.h"]
        );
        assert_eq!(modules(&extraction), ["lib/util.h"]);
    }

    #[test]
    fn quoted_include_prefers_the_including_files_directory() {
        let files = [
            "drv/dx11/driver.h",
            "drv/dx12/driver.h",
            "drv/vulkan/driver.h",
            "drv/vulkan/impl.cpp",
            "drv/common/impl.cpp",
        ];
        let source = b"#include \"driver.h\"\n#include <driver.h>\nvoid run() {}\n";

        let beside = resolved(source, "drv/vulkan/impl.cpp", &files);
        let apart = resolved(source, "drv/common/impl.cpp", &files);

        assert_eq!(
            targets(&beside),
            ["drv/vulkan/driver.h", "drv/vulkan/driver.h"]
        );
        assert_eq!(targets(&apart), Vec::<&str>::new());
        assert_eq!(modules(&apart), ["driver.h", "driver.h"]);
    }

    #[test]
    fn an_include_several_copies_end_in_names_the_copy_that_holds_the_includer() {
        let files = [
            "racing/main/app.h",
            "flight/main/app.h",
            "flight/render/scene.cpp",
            "libs/zlib/zlib.h",
            "libs/zlib/contrib/zlib.h",
            "vendor/zlib/zlib.h",
        ];
        let source = b"#include \"main/app.h\"\n#include <zlib.h>\nvoid run() {}\n";

        let inside = resolved(source, "flight/render/scene.cpp", &files);
        let nested = resolved(source, "libs/zlib/contrib/inflate.c", &files);
        let outside = resolved(source, "game/scene.cpp", &files);

        assert_eq!(targets(&inside), ["flight/main/app.h"]);
        assert_eq!(modules(&inside), ["zlib.h"]);
        assert_eq!(targets(&nested), ["libs/zlib/contrib/zlib.h"]);
        assert_eq!(modules(&nested), ["main/app.h"]);
        assert_eq!(targets(&outside), Vec::<&str>::new());
        assert_eq!(modules(&outside), ["main/app.h", "zlib.h"]);
    }

    #[test]
    fn exact_include_match_beats_earlier_suffix_match() {
        let extraction = resolved(
            b"#include \"widget/utils.h\"\nvoid run() {}\n",
            "widget/main.cpp",
            &["app/widget/utils.h", "widget/utils.h", "widget/main.cpp"],
        );

        assert_eq!(targets(&extraction), ["widget/utils.h"]);
    }

    #[test]
    fn a_relative_include_resolves_from_the_including_files_directory_and_probes_extensions() {
        let extraction = resolved(
            b"#include \"../common/math\"\n#include \"./local.h\"\n#include \"../../../outside.h\"\nvoid run() {}\n",
            "src/render/scene.cpp",
            &["src/common/math.hpp", "src/render/local.h", "outside.h"],
        );

        assert_eq!(
            targets(&extraction),
            ["src/common/math.hpp", "src/render/local.h"]
        );
        assert_eq!(modules(&extraction), ["../../../outside.h"]);
    }

    #[test]
    fn a_specifier_without_a_candidate_of_its_name_probes_header_extensions() {
        let extraction = resolved(
            b"#include <core/config>\nvoid run() {}\n",
            "app/main.cpp",
            &["core/config.h"],
        );

        assert_eq!(targets(&extraction), ["core/config.h"]);
    }

    #[test]
    fn macro_includes_emit_no_import() {
        let extraction = resolved(
            b"#include SOME_MACRO\n#include <vector>\nvoid anchor() {}\n",
            "src/inc.cpp",
            &[],
        );

        assert_eq!(targets(&extraction), Vec::<&str>::new());
        assert_eq!(modules(&extraction), ["vector"]);
    }

    #[test]
    fn test_files_mark_every_symbol_as_test() {
        let source = b"\nvoid helper() {}\n";

        for path in [
            "src/fooTest.cpp",
            "src/foo_test.cc",
            "src/foo_unittest.cpp",
            "src/test_foo.cpp",
            "app/test/a.cpp",
            "app/tests/b.cpp",
            "app/unittest/c.cpp",
            "prog/unittests/x.cpp",
        ] {
            let extraction = resolved(source, path, &[]);
            assert!(
                extraction
                    .symbols
                    .iter()
                    .all(|symbol| symbol.flags.is_test()),
                "{path}"
            );
        }
        let production = resolved(source, "prog/engine/foo.cpp", &[]);
        assert!(!production.symbols[0].flags.is_test());
    }

    #[test]
    fn generated_file_names_flag_every_symbol_as_generated() {
        let source = b"\nvoid emitted() {}\n";

        for path in [
            "src/foo.pb.cc",
            "src/foo.pb.cpp",
            "src/foo.pb.h",
            "src/foo.pb.hpp",
            "src/foo.pb.cxx",
            "src/foo.pb.hxx",
            "src/foo.pb.hh",
            "src/foo.pb.inl",
            "src/foo.pb.inc",
            "src/schema_generated.h",
            "src/foo.gen.cpp",
            "src/foo_gen.cpp",
            "src/foo_AOT.cpp",
        ] {
            let extraction = resolved(source, path, &[]);
            assert!(extraction.symbols[0].flags.is_generated(), "{path}");
        }
        let hand_written = resolved(source, "src/foo.cpp", &[]);
        assert!(!hand_written.symbols[0].flags.is_generated());
    }

    #[test]
    fn a_file_scope_constant_of_a_c_file_is_exported_unless_static() {
        let source = b"const int limit = 4;\nstatic const int local_limit = 2;\nconst static int other = 1;\n\
                       void use_them(void) { const int inner = 3; }\n";

        let exported = |path: &str| -> Vec<(String, bool)> {
            resolved(source, path, &[])
                .symbols
                .iter()
                .filter(|symbol| symbol.kind == SymbolKind::Constant)
                .map(|symbol| (symbol.name.clone(), symbol.exported))
                .collect()
        };

        let in_c = exported("src/limits.c");
        assert_eq!(
            in_c,
            [
                ("limit".to_owned(), true),
                ("local_limit".to_owned(), false),
                ("other".to_owned(), false)
            ]
        );
        assert_eq!(exported("src/LIMITS.C"), in_c);
        let unexported = [
            ("limit".to_owned(), false),
            ("local_limit".to_owned(), false),
            ("other".to_owned(), false),
        ];
        assert_eq!(exported("src/limits.cpp"), unexported);
        assert_eq!(exported("src/limits.h"), unexported);
    }

    #[test]
    fn path_flags_add_to_what_the_bytes_showed() {
        let extraction = resolved(
            b"// @generated by tool\nint main() { return 0; }\n",
            "tools/tests/gen.cpp",
            &[],
        );

        let flags = extraction.symbols[0].flags;
        assert!(flags.is_generated());
        assert!(flags.is_entry());
        assert!(flags.is_test());
    }
}
