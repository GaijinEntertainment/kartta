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

const PARENT: &str = "\
class Base:
    def size(self):
        return 1

    def shape(self):
        return 2

";

/// `app.Child.run`'s callees where `Child(Base)` opens with `body`, which holds whatever writes `size`, and `run` calls
/// `self.size()` and `self.shape()`, a member nothing writes, in that order.
fn callees_under(body: &str) -> Vec<(String, u8)> {
    let run = "    def run(self):\n        self.size()\n        self.shape()\n";
    callees(
        &format!("{PARENT}class Child(Base):\n{body}\n{run}"),
        "app.Child.run",
    )
}

fn shape_alone() -> Vec<(String, u8)> {
    vec![("app.Base.shape".to_owned(), 85)]
}

fn size_and_shape() -> Vec<(String, u8)> {
    vec![
        ("app.Base.size".to_owned(), 85),
        ("app.Base.shape".to_owned(), 85),
    ]
}

#[test]
fn a_value_the_class_writes_hides_the_parent_s_method() {
    assert_eq!(callees_under("    size = 3\n"), shape_alone());
}

#[test]
fn a_function_the_class_writes_hides_the_parent_s_method() {
    assert_eq!(
        callees_under("    size = len\n    def other(self):\n        pass\n"),
        shape_alone()
    );
}

#[test]
fn an_annotated_value_the_class_writes_hides_the_parent_s_method() {
    assert_eq!(callees_under("    size: int = 3\n"), shape_alone());
}

#[test]
fn a_property_the_class_writes_hides_the_parent_s_method() {
    assert_eq!(
        callees_under("    size = property(lambda self: 1)\n"),
        shape_alone()
    );
}

#[test]
fn an_attribute_an_instance_is_given_hides_the_parent_s_method_on_self() {
    assert_eq!(
        callees_under("    def __init__(self):\n        self.size = 3\n"),
        shape_alone()
    );
}

#[test]
fn an_annotation_without_a_value_leaves_the_parent_s_method() {
    assert_eq!(callees_under("    size: int\n"), size_and_shape());
}

#[test]
fn a_class_that_writes_no_attribute_of_the_name_keeps_the_parent_s_method() {
    assert_eq!(callees_under("    width = 3\n"), size_and_shape());
}

#[test]
fn a_call_a_hidden_method_turns_away_binds_no_namesake() {
    let namesake = "class Stranger:\n    def size(self):\n        pass\n\n";
    let source = format!(
        "{PARENT}{namesake}class Child(Base):\n    size = 3\n\n    def run(self):\n        self.size()\n"
    );

    assert_eq!(callees(&source, "app.Child.run"), []);
}

const ATTRIBUTE_ON_THE_WAY: &str = "\
class Top:
    def size(self):
        pass

class Middle(Top):
    size = 3

class Child(Middle):
    def run(self):
        self.size()

    def on_the_class(self):
        Child.size()
";

#[test]
fn an_attribute_between_the_class_and_the_method_hides_the_method() {
    assert_eq!(callees(ATTRIBUTE_ON_THE_WAY, "app.Child.run"), []);
    assert_eq!(callees(ATTRIBUTE_ON_THE_WAY, "app.Child.on_the_class"), []);
}

const METHOD_BELOW_THE_ATTRIBUTE: &str = "\
class Top:
    size = 3

class Middle(Top):
    def size(self):
        pass

class Child(Middle):
    def run(self):
        self.size()
";

#[test]
fn a_method_below_an_attribute_answers() {
    assert_eq!(
        callees(METHOD_BELOW_THE_ATTRIBUTE, "app.Child.run"),
        [("app.Middle.size".to_owned(), 85)]
    );
}

const TWO_PARENTS: &str = "\
class Holder:
    size = 3

class Sized:
    def size(self):
        pass

class HolderFirst(Holder, Sized):
    def run(self):
        self.size()

class SizedFirst(Sized, Holder):
    def run(self):
        self.size()
";

#[test]
fn the_earlier_of_two_parents_answers_with_what_it_holds() {
    assert_eq!(callees(TWO_PARENTS, "app.HolderFirst.run"), []);
    assert_eq!(
        callees(TWO_PARENTS, "app.SizedFirst.run"),
        [("app.Sized.size".to_owned(), 85)]
    );
}

#[test]
fn an_attribute_of_a_class_nested_in_the_class_hides_nothing() {
    assert_eq!(
        callees_under("    class Inner:\n        size = 3\n"),
        size_and_shape()
    );
}

#[test]
fn an_attribute_of_a_namesake_of_the_class_hides_nothing() {
    let source = format!(
        "{PARENT}if flag:
    class Child:
        size = 3
else:
    class Child(Base):
        def run(self):
            self.size()
"
    );

    assert_eq!(
        callees(&source, "app.Child.run"),
        [("app.Base.size".to_owned(), 85)]
    );
}

#[test]
fn an_attribute_with_no_method_above_it_binds_no_namesake() {
    let source = "\
class Stranger:
    def size(self):
        pass

class Child:
    size = 3

    def run(self):
        self.size()
";

    assert_eq!(callees(source, "app.Child.run"), []);
}

const LONE_CLASS: &str = "\
from other import Other

class Lone:
    def __init__(self, fn):
        self.size = fn

    def run(self):
        return self.size()
";

const OTHER_CLASS: &str = "\
class Other:
    def size(self):
        return 2
";

#[test]
fn an_attribute_an_instance_is_given_binds_no_namesake_of_another_file() {
    assert_eq!(
        callees_in(
            &[("app.py", LONE_CLASS), ("other.py", OTHER_CLASS)],
            "app.Lone.run"
        ),
        []
    );
}

const SIZE_OF_A_STRANGER: &str = "\
class Stranger:
    def size(self):
        pass

";

#[test]
fn an_attribute_of_a_parent_with_no_method_above_it_binds_no_namesake() {
    let source = format!(
        "{SIZE_OF_A_STRANGER}class Holder:
    def __init__(self, fn):
        self.size = fn

class Child(Holder):
    def run(self):
        self.size()
"
    );

    assert_eq!(callees(&source, "app.Child.run"), []);
}

#[test]
fn an_attribute_of_a_class_with_a_parent_outside_the_graph_binds_no_namesake() {
    let source = format!(
        "import outside

{SIZE_OF_A_STRANGER}class Child(outside.Base):
    def __init__(self, fn):
        self.size = fn

    def run(self):
        self.size()
"
    );

    assert_eq!(callees(&source, "app.Child.run"), []);
}

#[test]
fn an_attribute_holding_a_function_of_the_name_binds_nothing() {
    let source = "\
def size():
    pass

class Child:
    def __init__(self):
        self.size = size

    def run(self):
        self.size()
";

    assert_eq!(callees(source, "app.Child.run"), []);
}

#[test]
fn an_attribute_on_a_call_naming_the_class_binds_no_namesake() {
    let source = format!(
        "{SIZE_OF_A_STRANGER}class Child:
    size = 3

    def run(self):
        Child.size()
"
    );

    assert_eq!(callees(&source, "app.Child.run"), []);
}

#[test]
fn an_annotation_without_a_value_leaves_the_call_to_the_tiers() {
    let source = format!(
        "{SIZE_OF_A_STRANGER}class Child:
    size: int

    def run(self):
        self.size()
"
    );

    assert_eq!(
        callees(&source, "app.Child.run"),
        [("app.Stranger.size".to_owned(), 80)]
    );
}

#[test]
fn an_attribute_a_subclass_writes_leaves_the_mixin_s_call_to_the_tiers() {
    let source = format!(
        "{SIZE_OF_A_STRANGER}class Mixin:
    def run(self):
        self.size()

class Concrete(Mixin):
    def __init__(self, fn):
        self.size = fn
"
    );

    assert_eq!(
        callees(&source, "app.Mixin.run"),
        [("app.Stranger.size".to_owned(), 80)]
    );
}
