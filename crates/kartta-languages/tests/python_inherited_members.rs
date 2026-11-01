//! What a Python call on a class the graph holds binds of the types above that class: the member the class does not
//! hold itself binds where its nearest parent holds it, on the caller's own receivers and on a class the call names.
#![expect(
    clippy::expect_used,
    reason = "clippy's allow-expect-in-tests only covers #[cfg(test)] modules, and the build helpers of an integration \
              test crate are neither; a failed build is the assertion"
)]

use std::fs;

use kartta::{BuildOptions, Engine, Limits};

/// The qualified name and the confidence of each callee of `caller`, in a tree that holds `source` as `app.py`.
fn callees(source: &str, caller: &str) -> Vec<(String, u8)> {
    callees_in(&[("app.py", source)], caller)
}

/// [`callees`] in a tree of several files.
fn callees_in(files: &[(&str, &str)], caller: &str) -> Vec<(String, u8)> {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, source) in files {
        fs::write(tree.path().join(path), source).expect("the file is written");
    }
    let languages = kartta_languages::select(["python"]).expect("the language is known");
    let outcome = Engine::build(tree.path(), &BuildOptions::default(), languages, None)
        .expect("the tree builds");
    assert!(outcome.errors.is_empty());
    let (_, graph) = outcome
        .engine
        .graph_of(files[0].0)
        .expect("Python claims the file");
    graph
        .callees(caller, Limits::new(1))
        .expect("the name is defined")
        .reached
        .iter()
        .map(|reached| (reached.symbol.qualified_name.to_owned(), reached.confidence))
        .collect()
}

const HIERARCHY: &str = "\
class Grandparent:
    def far(self):
        pass

    def shadowed(self):
        pass

class Stranger:
    def far(self):
        pass

    def shadowed(self):
        pass

class Middle(Grandparent):
    def shadowed(self):
        pass

class Leaf(Middle):
    def on_self(self):
        self.far()

    def on_cls(self):
        cls.shadowed()

    def on_named(self):
        Leaf.far()

class Other:
    def far(self):
        pass

    def on_parent_class(self):
        Leaf.shadowed()
";

#[test]
fn a_call_on_self_binds_the_member_the_nearest_parent_holds() {
    assert_eq!(
        callees(HIERARCHY, "app.Leaf.on_self"),
        [("app.Grandparent.far".to_owned(), 85)]
    );
}

#[test]
fn a_call_on_cls_stops_at_the_first_parent_that_holds_the_member() {
    assert_eq!(
        callees(HIERARCHY, "app.Leaf.on_cls"),
        [("app.Middle.shadowed".to_owned(), 85)]
    );
}

#[test]
fn a_call_on_a_named_class_binds_its_parent_s_member() {
    assert_eq!(
        callees(HIERARCHY, "app.Leaf.on_named"),
        [("app.Grandparent.far".to_owned(), 85)]
    );
}

#[test]
fn a_call_on_a_named_class_binds_past_the_caller_s_own_namesake() {
    assert_eq!(
        callees(HIERARCHY, "app.Other.on_parent_class"),
        [("app.Middle.shadowed".to_owned(), 85)]
    );
}

const BASE: &str = "\
class Base:
    def inherited(self):
        pass
";

const STRANGER: &str = "\
class Stranger:
    def inherited(self):
        pass
";

#[test]
fn a_parent_the_module_imports_by_name_hands_its_member_on() {
    let child = "\
from base import Base

class Child(Base):
    def run(self):
        self.inherited()
";

    assert_eq!(
        callees_in(
            &[
                ("child.py", child),
                ("base.py", BASE),
                ("stranger.py", STRANGER)
            ],
            "child.Child.run"
        ),
        [("base.Base.inherited".to_owned(), 85)]
    );
}

#[test]
fn a_parent_written_through_its_module_hands_its_member_on() {
    let child = "\
import base as b

class Child(b.Base):
    def run(self):
        self.inherited()
";

    assert_eq!(
        callees_in(
            &[
                ("child.py", child),
                ("base.py", BASE),
                ("stranger.py", STRANGER)
            ],
            "child.Child.run"
        ),
        [("base.Base.inherited".to_owned(), 85)]
    );
}

#[test]
fn a_parent_imported_from_outside_the_repository_is_no_namesake_an_import_reaches() {
    let child = "\
from django.views import Base
import helpers

class Child(Base):
    def run(self):
        self.inherited()
";

    assert_eq!(
        callees_in(
            &[
                ("child.py", child),
                ("helpers.py", "from base import Base\n"),
                ("base.py", BASE),
                ("stranger.py", STRANGER),
            ],
            "child.Child.run"
        ),
        []
    );
}

#[test]
fn a_parent_written_through_an_outside_module_is_no_namesake_of_an_imported_file() {
    let child = "\
from models import Base
import django.db as db

class Child(db.Base):
    def run(self):
        self.inherited()
";

    assert_eq!(
        callees_in(
            &[
                ("child.py", child),
                ("models.py", BASE),
                ("other.py", STRANGER)
            ],
            "child.Child.run"
        ),
        [("models.Base.inherited".to_owned(), 70)]
    );
}

#[test]
fn a_parent_no_import_reaches_hands_nothing_on() {
    let child = "\
class Child(Base):
    def run(self):
        self.inherited()
";

    assert_eq!(
        callees_in(
            &[
                ("child.py", child),
                ("base.py", BASE),
                ("stranger.py", STRANGER)
            ],
            "child.Child.run"
        ),
        []
    );
}

const PARENTED: &str = "\
class Parent:
    def inherited(self):
        pass

class Named(Parent):
    pass
";

#[test]
fn a_class_no_import_reaches_is_no_receiver_s_class() {
    let caller = "\
def run():
    Named.inherited()
";

    assert_eq!(
        callees_in(
            &[
                ("caller.py", caller),
                ("named.py", PARENTED),
                ("other.py", STRANGER)
            ],
            "caller.run"
        ),
        []
    );
}

#[test]
fn a_class_imported_from_outside_is_no_namesake_receiver() {
    let caller = "\
from django.views import Named
import helpers

def run():
    Named.inherited()
";

    assert_eq!(
        callees_in(
            &[
                ("caller.py", caller),
                ("helpers.py", "from named import Named\n"),
                ("named.py", PARENTED),
                ("other.py", STRANGER)
            ],
            "caller.run"
        ),
        []
    );
}

const DIAMOND: &str = "\
class Top:
    def member(self):
        pass

class Left(Top):
    pass

class Right(Top):
    def member(self):
        pass

class Bottom(Left, Right):
    def run(self):
        self.member()
";

#[test]
fn a_diamond_binds_the_side_that_overrides_ahead_of_the_shared_base() {
    assert_eq!(
        callees(DIAMOND, "app.Bottom.run"),
        [("app.Right.member".to_owned(), 85)]
    );
}

const FIRST_BASE: &str = "\
class Second:
    def member(self):
        pass

class First:
    def member(self):
        pass

class Child(First, Second):
    def run(self):
        self.member()
";

#[test]
fn a_nearer_base_written_first_answers_ahead_of_the_second() {
    assert_eq!(
        callees(FIRST_BASE, "app.Child.run"),
        [("app.First.member".to_owned(), 85)]
    );
}

const OWN_MEMBER: &str = "\
class Parent:
    def member(self):
        pass

class Child(Parent):
    def member(self):
        pass

    def run(self):
        self.member()
";

#[test]
fn a_member_the_class_holds_stays_with_the_caller_s_owner_tier() {
    assert_eq!(
        callees(OWN_MEMBER, "app.Child.run"),
        [("app.Child.member".to_owned(), 80)]
    );
}

#[test]
fn a_namesake_of_the_parent_holds_nothing_for_it() {
    let child = "\
from empty import Base

class Child(Base):
    def run(self):
        self.inherited()
";

    assert_eq!(
        callees_in(
            &[
                ("child.py", child),
                ("empty.py", "class Base:\n    pass\n"),
                ("base.py", BASE),
                ("stranger.py", STRANGER)
            ],
            "child.Child.run"
        ),
        []
    );
}

const NESTED: &str = "\
class Base:
    def member(self):
        pass

class Outer:
    class Inner(Base):
        def member(self):
            pass

class Child(Outer.Inner):
    def run(self):
        self.member()
";

#[test]
fn a_nested_parent_that_overrides_the_member_answers_ahead_of_its_own_parent() {
    assert_eq!(
        callees(NESTED, "app.Child.run"),
        [("app.Outer.Inner.member".to_owned(), 85)]
    );
}
