//! The graph through its public queries, one module per query, on extractions written by hand.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests covers #[test] functions and #[cfg(test)] modules, and the query helpers \
              of an integration test crate are neither; a name that selects nothing is the failure"
)]

mod analyze;
mod assertions;
mod calls;
mod contract;
mod dead_code;
mod dependents;
mod go_code;
mod implementations;
mod members;
mod one_edit;
mod packages;
mod resolution;
mod search;
mod stats;
mod typed;

use kartta_graph::Graph;
use kartta_lang::{CallRef, Conventions, Extraction, IncludeRef, Symbol, SymbolId, SymbolKind};

fn graph_of(conventions: Conventions, files: &[Extraction]) -> Graph {
    let mut graph = Graph::new(conventions);
    for extraction in files {
        graph.insert(extraction);
    }
    graph
}

fn graph(files: &[Extraction]) -> Graph {
    graph_of(Conventions::default(), files)
}

fn file(path: &str, symbols: Vec<Symbol>) -> Extraction {
    let mut extraction = Extraction::default();
    path.clone_into(&mut extraction.file);
    extraction.symbols = symbols;
    extraction
}

fn including(mut extraction: Extraction, targets: &[&str]) -> Extraction {
    extraction.includes = targets
        .iter()
        .map(|&target| IncludeRef {
            target: target.to_owned(),
        })
        .collect();
    extraction
}

fn function(name: &str, line: u32) -> Symbol {
    Symbol::new(name, SymbolKind::Function, line, line + 1)
}

fn call(from: u32, callee: &str, line: u32) -> CallRef {
    CallRef {
        site_of: SymbolId(from),
        callee: callee.to_owned(),
        receiver: None,
        line,
        invokes_a_macro: false,
    }
}

fn member_call(receiver: &str, callee: &str, line: u32) -> CallRef {
    CallRef {
        receiver: Some(receiver.to_owned()),
        ..call(0, callee, line)
    }
}
