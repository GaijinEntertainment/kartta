use kartta_graph::{Graph, Limits};
use kartta_lang::{CallRef, Extraction, ImportBinding, Symbol, SymbolKind};

pub(crate) fn function(package: &str, name: &str, line: u32) -> Symbol {
    let mut symbol = Symbol::new(name, SymbolKind::Function, line, line + 1);
    symbol.qualified_name = format!("{package}.{name}");
    symbol.exported = true;
    symbol
}

pub(crate) fn file(
    path: &str,
    package: &str,
    symbols: Vec<Symbol>,
    calls: Vec<CallRef>,
) -> Extraction {
    let mut extraction = crate::file(path, symbols);
    extraction.package = Some(package.to_owned());
    extraction.calls = calls;
    extraction
}

pub(crate) fn importing(
    mut extraction: Extraction,
    imports: &[(&str, Option<&str>)],
) -> Extraction {
    extraction.bindings = imports
        .iter()
        .map(|&(name, target_dir)| {
            ImportBinding::new(None, target_dir.map(str::to_owned), name.to_owned())
        })
        .collect();
    extraction
}

pub(crate) fn callees_of(graph: &Graph, name: &str) -> Vec<(String, u8)> {
    let walk = graph
        .callees(name, Limits::new(1))
        .expect("the caller is held");
    walk.reached
        .iter()
        .map(|reached| {
            (
                format!("{}:{}", reached.symbol.file, reached.symbol.qualified_name),
                reached.confidence,
            )
        })
        .collect()
}
