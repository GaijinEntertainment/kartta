mod extraction;
mod identity;
mod resolution;
mod uses;

use std::io;

use kartta_lang::path::{base, dir};
use kartta_lang::{Extraction, Language, Repository, Symbol};

use crate::{JavaScript, Tsx, TypeScript};

pub(crate) struct Files {
    files: Vec<(String, Vec<u8>)>,
}

impl Files {
    pub(crate) fn new(files: &[(&str, &str)]) -> Self {
        Self {
            files: (files.iter())
                .map(|(path, contents)| ((*path).to_owned(), contents.as_bytes().to_vec()))
                .collect(),
        }
    }

    pub(crate) fn paths(paths: &[&str]) -> Self {
        Self {
            files: paths
                .iter()
                .map(|path| ((*path).to_owned(), Vec::new()))
                .collect(),
        }
    }
}

impl Repository for Files {
    fn contains(&self, path: &str) -> bool {
        self.files.iter().any(|(file, _)| file == path)
    }

    fn files_in_dir<'a>(&'a self, wanted: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let wanted = wanted.to_owned();
        Box::new(
            (self.files.iter())
                .map(|(file, _)| file.as_str())
                .filter(move |file| dir(file) == wanted)
                .map(str::to_owned),
        )
    }

    fn files_named<'a>(&'a self, name: &str) -> Box<dyn Iterator<Item = String> + 'a> {
        let name = name.to_owned();
        Box::new(
            (self.files.iter())
                .map(|(file, _)| file.as_str())
                .filter(move |file| base(file) == name)
                .map(str::to_owned),
        )
    }

    fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        (self.files.iter())
            .find(|(file, _)| file == path)
            .map(|(_, contents)| contents.clone())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

pub(crate) const LANGUAGES: [&dyn Language; 3] =
    [&TypeScript::new(), &Tsx::new(), &JavaScript::new()];

pub(crate) fn language_of(path: &str) -> &'static dyn Language {
    (LANGUAGES.into_iter())
        .find(|language| language.claims(path))
        .unwrap_or_else(|| panic!("no language claims {path}"))
}

pub(crate) fn resolved(path: &str, source: &str, files: &Files) -> Extraction {
    let language = language_of(path);
    language
        .resolver(files)
        .resolve(language.extract(source.as_bytes()), path)
}

pub(crate) fn extracted(path: &str, source: &str) -> Extraction {
    resolved(path, source, &Files::paths(&[path]))
}

pub(crate) fn symbol<'e>(extraction: &'e Extraction, name: &str) -> &'e Symbol {
    (extraction.symbols.iter())
        .find(|symbol| symbol.name == name)
        .unwrap_or_else(|| panic!("no symbol {name}"))
}

pub(crate) fn names(extraction: &Extraction) -> Vec<&str> {
    extraction
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect()
}

pub(crate) fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

pub(crate) fn imports(extraction: &Extraction) -> Vec<&str> {
    extraction
        .imports
        .iter()
        .map(|import| import.module.as_str())
        .collect()
}

pub(crate) fn attributed(extraction: &Extraction) -> Vec<(&str, &str)> {
    (extraction.calls.iter())
        .map(|call| {
            (
                extraction.symbols[call.caller_id.0 as usize].name.as_str(),
                call.callee.as_str(),
            )
        })
        .collect()
}

pub(crate) fn caller_of<'e>(extraction: &'e Extraction, callee: &str) -> &'e Symbol {
    let call = (extraction.calls.iter())
        .find(|call| call.callee == callee)
        .unwrap_or_else(|| panic!("no call of {callee}"));
    (extraction.symbols.get(call.caller_id.0 as usize))
        .unwrap_or_else(|| panic!("{callee} is called at file scope"))
}

pub(crate) const SAMPLE_TS: &str = r#"
import { helper } from "./util";

export interface Shape {
    area(): number;
}

export type Alias = Shape;

export enum Color { Red, Green }

export const LIMIT = 10;

export class Circle extends Base implements Shape {
    area(): number {
        return compute(this.r);
    }
}

export function render(c: Circle): void {
    c.area();
}

export { helper as help, other } from "./more";
export * from "./everything";
"#;
