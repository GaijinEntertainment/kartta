use std::any::Any;

use kartta_lang::path::base;
use kartta_lang::{
    Extraction, ImportBinding, ImportRef, IncludeRef, RawExtraction, Repository, Resolver,
    SymbolFlags,
};

use crate::config::is_vendored;
use crate::generated::name_marks_generated;
use crate::modules::{Configs, Modules};

#[derive(PartialEq)]
pub(crate) struct JstsResolver {
    configs: Configs,
}

impl JstsResolver {
    pub(crate) fn new(repository: &dyn Repository) -> Self {
        Self {
            configs: Configs::read(repository),
        }
    }
}

impl Resolver for JstsResolver {
    fn resolve(&self, raw: RawExtraction, path: &str, repository: &dyn Repository) -> Extraction {
        let modules = Modules {
            repository,
            configs: &self.configs,
        };
        let (mut extraction, directives) = raw.into_extraction(path);

        let path_flags = SymbolFlags::empty()
            .with_test(is_test_file(path))
            .with_generated(name_marks_generated(base(path)))
            .with_vendored(is_vendored(path));
        extraction.flag_symbols(path_flags);
        for callable in &mut extraction.callables {
            callable.key = format!("{path}#{}", callable.key);
        }

        // constraint: the walk records re-exports in the order of their statements' directives
        let mut re_exports = extraction.re_exports.iter_mut().peekable();
        for directive in directives {
            let target = modules.resolve(&directive.spec, path);
            while let Some(re_export) =
                re_exports.next_if(|re_export| re_export.source_path == directive.spec)
            {
                re_export.source_file.clone_from(&target);
            }

            match target {
                Some(target) => extraction.includes.push(IncludeRef { target }),
                None if !directive.spec.is_empty() => {
                    bind_outside(&mut extraction.bindings, &directive.spec, directive.names);
                    extraction.imports.push(ImportRef {
                        module: directive.spec,
                        names: Vec::new(),
                        line: directive.line,
                    });
                }
                None => {}
            }
        }

        extraction
    }

    fn resolves_as(&self, kept: &dyn Resolver) -> bool {
        (kept as &dyn Any).downcast_ref::<Self>() == Some(self)
    }
}

// constraint: a name imported from a module outside the repository holds that module's code, so a member call through
// it, `<Dialog.Footer>` or `z.object()`, binds no definition of the repository
fn bind_outside(bindings: &mut Vec<ImportBinding>, spec: &str, names: Vec<String>) {
    let last_segment = spec.rsplit('/').next().unwrap_or(spec);
    bindings.extend(names.into_iter().map(|name| ImportBinding {
        alias: Some(name),
        target_dir: None,
        last_segment: last_segment.to_owned(),
    }));
}

fn is_test_file(path: &str) -> bool {
    let name = base(path);
    name.contains(".test.")
        || name.contains(".spec.")
        || path
            .split('/')
            .any(|component| component == "__tests__" || component == "tests")
}
