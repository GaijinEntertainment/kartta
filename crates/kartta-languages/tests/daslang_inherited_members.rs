//! What a daslang call on a class the graph holds binds of the classes above it: a class starts as a copy of its
//! parent's fields, so the member it does not hold itself is the one its nearest parent holds, an override included.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

/// The qualified name and the confidence of each callee of `caller`, in a tree of `files`.
fn callees(files: &[(&str, &str)], caller: &str) -> Vec<(String, u8)> {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        fs::write(tree.path().join(path), source).expect("the file is written");
    }
    let languages = kartta_languages::select(["daslang"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of(files[0].0)
        .expect("daslang claims the file");
    graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

#[test]
fn a_call_on_the_own_class_binds_the_member_its_nearest_parent_holds() {
    let source = "options gen2
class Root {
    def reset() {
    }
}
class Other {
    def reset() {
    }
}
class Mid : Root {
}
class Leaf : Mid {
    def tick() {
        self->reset()
    }
}
";

    assert_eq!(
        callees(&[("a.das", source)], "Leaf.tick"),
        [("Root.reset".to_owned(), 85)],
    );
}

#[test]
fn an_override_nearer_the_class_stands_ahead_of_the_member_it_replaces() {
    let source = "options gen2
class Root {
    def reset() {
    }
}
class Mid : Root {
    def override reset() {
    }
}
class Leaf : Mid {
    def tick() {
        self.reset()
    }
}
";

    assert_eq!(
        callees(&[("a.das", source)], "Leaf.tick"),
        [("Mid.reset".to_owned(), 85)],
    );
}

#[test]
fn a_call_on_a_class_that_overrides_the_member_stays_on_the_class() {
    let source = "options gen2
class Root {
    def reset() {
    }
}
class Leaf : Root {
    def override reset() {
    }
    def tick() {
        self->reset()
    }
}
";

    assert_eq!(
        callees(&[("a.das", source)], "Leaf.tick"),
        [("Leaf.reset".to_owned(), 80)],
    );
}

#[test]
fn a_parent_of_a_required_module_hands_its_members_on() {
    let leaf = "options gen2
require base
class Leaf : base::Root {
    def tick() {
        self->reset()
    }
}
class Other {
    def reset() {
    }
}
";
    let base = "options gen2
module base
class Root {
    def reset() {
    }
}
";

    assert_eq!(
        callees(&[("leaf.das", leaf), ("base.das", base)], "Leaf.tick"),
        [("base.Root.reset".to_owned(), 85)],
    );
}
