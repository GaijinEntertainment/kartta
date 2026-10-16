//! How a Python extraction resolves against its file's path and the repository: the module that qualifies its
//! symbols, the files its imports include, and the test and generated code a path marks.

use std::io;

use kartta_lang::path::{base, dir};
use kartta_lang::{Extraction, Language, Repository};
use kartta_lang_python::Python;

struct Files(&'static [&'static str]);

impl Repository for Files {
    fn contains(&self, path: &str) -> bool {
        self.0.contains(&path)
    }

    fn files_in_dir<'a>(&'a self, in_dir: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let in_dir = in_dir.to_owned();
        Box::new(
            self.0
                .iter()
                .filter(move |file| dir(file) == in_dir)
                .map(|file| (*file).to_owned()),
        )
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let name = name.to_owned();
        Box::new(
            self.0
                .iter()
                .filter(move |file| base(file) == name)
                .map(|file| (*file).to_owned()),
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        Err(io::Error::new(io::ErrorKind::NotFound, path.to_owned()))
    }
}

fn resolve(files: &Files, source: &[u8], path: &str) -> Extraction {
    let python = Python::new();
    let resolver = python.resolver(files);

    resolver.resolve(python.extract(source), path)
}

fn includes(files: &'static [&'static str], source: &[u8], path: &str) -> Vec<String> {
    resolve(&Files(files), source, path)
        .includes
        .into_iter()
        .map(|include| include.target)
        .collect()
}

const DEFINITION: &[u8] = b"def anchor():\n    pass\n";

#[test]
fn every_symbol_is_qualified_by_the_module_its_path_names() {
    let extraction = resolve(
        &Files(&[]),
        b"VERSION = 1\nclass Service:\n    def greet(self):\n        pass\n",
        "pkg/service.py",
    );
    let names: Vec<&str> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();

    assert_eq!(
        names,
        [
            "pkg.service.VERSION",
            "pkg.service.Service",
            "pkg.service.Service.greet"
        ]
    );
    assert_eq!(extraction.symbols[2].owner.as_deref(), Some("Service"));
}

#[test]
fn a_stub_names_the_module_of_its_source() {
    let extraction = resolve(&Files(&[]), DEFINITION, "pkg/api.pyi");

    assert_eq!(extraction.symbols[0].qualified_name, "pkg.api.anchor");
}

#[test]
fn a_claimed_extension_in_any_case_is_no_part_of_the_module_name() {
    for path in ["pkg/stub.PYI", "pkg/stub.Py"] {
        let extraction = resolve(&Files(&[]), DEFINITION, path);

        assert_eq!(
            extraction.symbols[0].qualified_name, "pkg.stub.anchor",
            "{path}"
        );
    }
}

#[test]
fn a_package_initializer_names_the_module_of_its_directory() {
    let package = resolve(&Files(&[]), DEFINITION, "pkg/sub/__init__.py");
    let root = resolve(&Files(&[]), DEFINITION, "__init__.pyi");
    let lookalike = resolve(&Files(&[]), DEFINITION, "pkg/my__init__.py");

    assert_eq!(package.symbols[0].qualified_name, "pkg.sub.anchor");
    assert_eq!(root.symbols[0].qualified_name, "anchor");
    assert_eq!(lookalike.symbols[0].qualified_name, "pkg.my__init__.anchor");
}

#[test]
fn an_import_that_names_a_file_is_an_include_and_any_other_is_kept_as_written() {
    let extraction = resolve(
        &Files(&[
            "pkg/service.py",
            "pkg/name.py",
            "pkg/sibling.py",
            "app/config.py",
        ]),
        b"import os\nimport app.config\nfrom . import sibling, name as aliased\n",
        "pkg/service.py",
    );
    let imports: Vec<(&str, Vec<&str>, u32)> = extraction
        .imports
        .iter()
        .map(|import| {
            (
                import.module.as_str(),
                import.names.iter().map(String::as_str).collect(),
                import.line,
            )
        })
        .collect();
    let targets: Vec<&str> = extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect();

    assert_eq!(imports, [("os", vec![], 1)]);
    assert_eq!(targets, ["app/config.py", "pkg/sibling.py", "pkg/name.py"]);
}

#[test]
fn a_future_import_is_recorded_and_never_probes_the_repository() {
    let extraction = resolve(
        &Files(&["__future__.py", "pkg/api.py"]),
        b"from __future__ import annotations\n\ndef anchor():\n    pass\n",
        "pkg/api.py",
    );

    assert_eq!(extraction.imports.len(), 1);
    assert_eq!(extraction.imports[0].module, "__future__");
    assert_eq!(extraction.imports[0].names, ["annotations"]);
    assert_eq!(extraction.includes.len(), 0);
}

#[test]
fn a_module_outside_the_repository_adds_no_include() {
    let targets = includes(
        &["pkg/service.py", "pkg/other.py"],
        b"import os\nfrom collections import OrderedDict\n",
        "pkg/service.py",
    );

    assert_eq!(targets, Vec::<String>::new());
}

#[test]
fn a_single_dot_imports_from_the_package_of_the_file() {
    let targets = includes(
        &["pkg/service.py", "pkg/name.py", "pkg/__init__.py"],
        b"from . import name\n",
        "pkg/service.py",
    );

    assert_eq!(targets, ["pkg/__init__.py", "pkg/name.py"]);
}

#[test]
fn a_relative_package_probes_its_names_as_submodules() {
    let targets = includes(
        &["pkg/service.py", "pkg/helpers/io.py"],
        b"from .helpers import io\n",
        "pkg/service.py",
    );

    assert_eq!(targets, ["pkg/helpers/io.py"]);
}

#[test]
fn each_dot_after_the_first_climbs_one_directory() {
    let shared = includes(
        &["pkg/sub/mod.py", "pkg/shared.py"],
        b"from .. import shared\n",
        "pkg/sub/mod.py",
    );
    let wildcard = includes(
        &["pkg/sub/mod.py", "pkg/util/__init__.py"],
        b"from ..util import *\n",
        "pkg/sub/mod.py",
    );

    assert_eq!(shared, ["pkg/shared.py"]);
    assert_eq!(wildcard, ["pkg/util/__init__.py"]);
}

#[test]
fn a_relative_import_above_the_repository_root_adds_no_include() {
    let targets = includes(
        &["shared.py", "pkg/mod.py"],
        b"from ... import shared\n",
        "pkg/mod.py",
    );

    assert_eq!(targets, Vec::<String>::new());
}

#[test]
fn a_relative_module_is_a_dependency_even_when_its_names_are_symbols() {
    let named = includes(
        &["pkg/service.py", "pkg/helpers.py"],
        b"from .helpers import fn\n",
        "pkg/service.py",
    );
    let wildcard = includes(
        &["pkg/main.py", "pkg/module.py"],
        b"from .module import *\n",
        "pkg/main.py",
    );

    assert_eq!(named, ["pkg/helpers.py"]);
    assert_eq!(wildcard, ["pkg/module.py"]);
}

#[test]
fn a_relative_import_of_a_plain_module_probes_no_submodules() {
    let targets = includes(
        &["pkg/main.py", "pkg/module.py", "pkg/module/thing.py"],
        b"from .module import thing\n",
        "pkg/main.py",
    );

    assert_eq!(targets, ["pkg/module.py"]);
}

#[test]
fn an_absolute_module_resolves_to_its_file_or_its_package() {
    let module = includes(
        &["main.py", "app/config.py"],
        b"import app.config\n",
        "main.py",
    );
    let package = includes(
        &["main.py", "app/pkg/__init__.py"],
        b"import app.pkg\n",
        "main.py",
    );

    assert_eq!(module, ["app/config.py"]);
    assert_eq!(package, ["app/pkg/__init__.py"]);
}

#[test]
fn an_absolute_module_falls_back_to_a_directory_around_the_importing_file() {
    let targets = includes(
        &["src/app/main.py", "src/app/db.py"],
        b"import app.db\n",
        "src/app/main.py",
    );

    assert_eq!(targets, ["src/app/db.py"]);
}

#[test]
fn the_repository_root_wins_over_a_directory_around_the_importing_file() {
    let targets = includes(
        &["app/db.py", "src/app/db.py", "src/app/main.py"],
        b"import app.db\n",
        "src/app/main.py",
    );

    assert_eq!(targets, ["app/db.py"]);
}

#[test]
fn a_from_import_of_a_package_includes_the_names_that_are_submodules() {
    let submodule = includes(
        &["main.py", "app/pkg/__init__.py", "app/pkg/child.py"],
        b"from app.pkg import child\n",
        "main.py",
    );
    let symbol = includes(
        &["main.py", "app/module.py"],
        b"from app.module import thing\n",
        "main.py",
    );
    let wildcard = includes(
        &["main.py", "app/module.py"],
        b"from app.module import *\n",
        "main.py",
    );

    assert_eq!(submodule, ["app/pkg/__init__.py", "app/pkg/child.py"]);
    assert_eq!(symbol, ["app/module.py"]);
    assert_eq!(wildcard, ["app/module.py"]);
}

#[test]
fn a_from_import_of_a_namespace_package_includes_the_names_that_are_submodules() {
    let at_root = includes(
        &["main.py", "app/child.py", "app/other.py"],
        b"from app import child, missing\n",
        "main.py",
    );
    let around = includes(
        &["src/main.py", "src/app/child.py", "lib/app/child.py"],
        b"from app import child\n",
        "src/main.py",
    );
    let nowhere = includes(
        &["main.py", "app/child.py"],
        b"from typing import Any\nimport app\n",
        "main.py",
    );

    assert_eq!(at_root, ["app/child.py"]);
    assert_eq!(around, ["src/app/child.py"]);
    assert_eq!(nowhere, Vec::<String>::new());
}

#[test]
fn a_module_the_repository_holds_only_as_a_stub_includes_the_stub() {
    let stub = includes(&["main.py", "api.pyi"], b"import api\n", "main.py");
    let package = includes(
        &["main.py", "types/__init__.pyi", "types/child.pyi"],
        b"from types import child\n",
        "main.py",
    );
    let source_first = includes(
        &["main.py", "api.py", "api.pyi"],
        b"import api\n",
        "main.py",
    );

    assert_eq!(stub, ["api.pyi"]);
    assert_eq!(package, ["types/__init__.pyi", "types/child.pyi"]);
    assert_eq!(source_first, ["api.py"]);
}

#[test]
fn a_from_import_of_a_plain_module_probes_no_submodules() {
    let targets = includes(
        &["main.py", "app/module.py", "app/module/thing.py"],
        b"from app.module import thing\n",
        "main.py",
    );

    assert_eq!(targets, ["app/module.py"]);
}

#[test]
fn a_test_path_marks_every_symbol_as_test_code() {
    let source = b"def helper():\n    pass\n\n\ndef test_thing():\n    pass\n";

    for path in [
        "test_foo.py",
        "foo_test.py",
        "pkg/conftest.py",
        "pkg/tests/util.py",
        "svc/test/run.py",
        "pkg/api_test.pyi",
    ] {
        let extraction = resolve(&Files(&[]), source, path);

        assert_eq!(extraction.symbols.len(), 2, "{path}");
        assert!(
            extraction
                .symbols
                .iter()
                .all(|symbol| symbol.flags.is_test()),
            "{path}"
        );
    }
    let ordinary = resolve(&Files(&[]), source, "pkg/foo.py");
    assert!(
        ordinary
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_test())
    );
}

#[test]
fn a_protobuf_module_or_its_stub_is_generated_code() {
    for path in [
        "pkg/foo_pb2.py",
        "pkg/foo_pb2_grpc.py",
        "gen/schema_pb2.pyi",
    ] {
        let extraction = resolve(&Files(&[]), DEFINITION, path);

        assert!(extraction.symbols[0].flags.is_generated(), "{path}");
    }
    assert!(
        !resolve(&Files(&[]), DEFINITION, "pkg/foo.py").symbols[0]
            .flags
            .is_generated()
    );
}

#[test]
fn a_test_path_keeps_the_flags_the_bytes_gave() {
    let extraction = resolve(
        &Files(&[]),
        b"# @generated\ndef main():\n    pass\n",
        "tests/gen.py",
    );
    let flags = extraction.symbols[0].flags;

    assert!(flags.is_test() && flags.is_generated() && flags.is_entry());
}

#[test]
fn one_extraction_resolved_at_two_paths_differs_only_by_what_the_paths_show() {
    let python = Python::new();
    let files = Files(&["pkg/util.py", "tests/util.py"]);
    let resolver = python.resolver(&files);
    let raw = python.extract(b"from . import util\ndef run():\n    pass\n");

    let ordinary = resolver.resolve(raw.clone(), "pkg/run.py");
    let test = resolver.resolve(raw, "tests/run.py");

    assert_eq!(ordinary.symbols[0].qualified_name, "pkg.run.run");
    assert_eq!(test.symbols[0].qualified_name, "tests.run.run");
    assert!(!ordinary.symbols[0].flags.is_test());
    assert!(test.symbols[0].flags.is_test());
    assert_eq!(ordinary.includes[0].target, "pkg/util.py");
    assert_eq!(test.includes[0].target, "tests/util.py");
    assert_eq!(ordinary.imports, test.imports);
}
