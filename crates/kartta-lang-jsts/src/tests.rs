mod extraction;
mod identity;
mod resolution;
mod uses;

use kartta_lang::{Extraction, Language, MemoryRepository, Symbol};

use crate::{JavaScript, Tsx, TypeScript};

pub(crate) const LANGUAGES: [&dyn Language; 3] =
    [&TypeScript::new(), &Tsx::new(), &JavaScript::new()];

pub(crate) fn language_of(path: &str) -> &'static dyn Language {
    (LANGUAGES.into_iter())
        .find(|language| language.claims(path))
        .unwrap_or_else(|| panic!("no language claims {path}"))
}

pub(crate) fn resolved(path: &str, source: &str, files: &MemoryRepository) -> Extraction {
    let language = language_of(path);
    language
        .resolver(files)
        .resolve(language.extract(source.as_bytes()), path)
}

pub(crate) fn extracted(path: &str, source: &str) -> Extraction {
    resolved(path, source, &MemoryRepository::of_paths([path]))
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
