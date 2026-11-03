//! What a Ruby file extracts to, how its requires resolve against the repository, and where the doc comment above a
//! definition starts.
#![expect(
    clippy::panic,
    reason = "clippy's allow-panic-in-tests covers #[test] functions and #[cfg(test)] modules, and the lookup \
              helpers of an integration test crate are neither; a failed lookup is the assertion"
)]

use kartta_lang::{
    Access, CallRef, CallableRole, Extraction, FILE_SCOPE, InheritedMembers, Language,
    MemoryRepository, OwnMembers, RawExtraction, Receiver, Symbol, SymbolId, SymbolKind, TypeUses,
};
use kartta_lang_ruby::Ruby;

const SAMPLE: &[u8] = br#"
require "set"

module Geometry
  class Shape
    include Comparable
    extend Mixins::Walkable

    attr_accessor :width, :height
    attr_reader :name

    MAX_SIDES = 12

    def area
      compute(width)
    end

    def self.build
      new
    end

    private

    def _secret
      0
    end
  end
end

class Circle < Shape
  def radius
    helper.draw
  end
end

def top_level
  puts("hi")
end
"#;

const WIDGET: &[u8] = b"\nclass Widget\n  def run\n    1\n  end\nend\n";

fn extract(source: &[u8]) -> RawExtraction {
    Ruby::new().extract(source)
}

fn resolve(source: &[u8], path: &str, files: &[&str]) -> Extraction {
    let ruby = Ruby::new();
    let repository = MemoryRepository::of_paths(files);
    let resolver = ruby.resolver(&repository);
    resolver.resolve(ruby.extract(source), path, &repository)
}

fn symbol<'r>(symbols: &'r [Symbol], qualified: &str) -> &'r Symbol {
    symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified)
        .unwrap_or_else(|| panic!("no symbol {qualified}"))
}

fn id_of(symbols: &[Symbol], qualified: &str) -> SymbolId {
    let at = symbols
        .iter()
        .position(|symbol| symbol.qualified_name == qualified)
        .unwrap_or_else(|| panic!("no symbol {qualified}"));
    SymbolId(u32::try_from(at).unwrap_or(u32::MAX))
}

fn call<'r>(calls: &'r [CallRef], callee: &str) -> &'r CallRef {
    calls
        .iter()
        .find(|call| call.callee == callee)
        .unwrap_or_else(|| panic!("no call of {callee}"))
}

fn calls(raw: &RawExtraction) -> Vec<(&str, Option<&str>)> {
    raw.calls
        .iter()
        .map(|call| (call.callee.as_str(), call.receiver.as_deref()))
        .collect()
}

fn names(symbols: &[Symbol]) -> Vec<&str> {
    symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect()
}

fn includes(extraction: &Extraction) -> Vec<&str> {
    extraction
        .includes
        .iter()
        .map(|include| include.target.as_str())
        .collect()
}

fn imports(extraction: &Extraction) -> Vec<&str> {
    extraction
        .imports
        .iter()
        .map(|import| import.module.as_str())
        .collect()
}

#[test]
fn every_definition_is_qualified_by_its_namespace_chain() {
    let raw = extract(SAMPLE);

    let symbols: Vec<(&str, SymbolKind)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect();
    assert_eq!(
        symbols,
        [
            ("Geometry", SymbolKind::Namespace),
            ("Geometry::Shape", SymbolKind::Class),
            ("Geometry::Shape#width", SymbolKind::Method),
            ("Geometry::Shape#width=", SymbolKind::Method),
            ("Geometry::Shape#height", SymbolKind::Method),
            ("Geometry::Shape#height=", SymbolKind::Method),
            ("Geometry::Shape#name", SymbolKind::Method),
            ("Geometry::Shape::MAX_SIDES", SymbolKind::Constant),
            ("Geometry::Shape#area", SymbolKind::Method),
            ("Geometry::Shape.build", SymbolKind::Method),
            ("Geometry::Shape#_secret", SymbolKind::Method),
            ("Circle", SymbolKind::Class),
            ("Circle#radius", SymbolKind::Method),
            ("top_level", SymbolKind::Function),
        ]
    );
}

#[test]
fn a_member_is_owned_by_its_type_and_a_top_level_def_by_nothing() {
    let raw = extract(SAMPLE);

    let area = symbol(&raw.symbols, "Geometry::Shape#area");
    assert_eq!(area.owner.as_deref(), Some("Geometry::Shape"));
    assert!(area.exported);
    let max = symbol(&raw.symbols, "Geometry::Shape::MAX_SIDES");
    assert_eq!(max.owner.as_deref(), Some("Geometry::Shape"));
    let build = symbol(&raw.symbols, "Geometry::Shape.build");
    assert_eq!(build.owner.as_deref(), Some("Geometry::Shape"));
    assert!(build.exported);
    assert_eq!(symbol(&raw.symbols, "top_level").owner, None);
}

#[test]
fn a_superclass_and_the_mixins_are_parents() {
    let raw = extract(SAMPLE);

    assert_eq!(
        symbol(&raw.symbols, "Geometry::Shape").parents,
        ["Comparable", "Mixins::Walkable"]
    );
    assert_eq!(symbol(&raw.symbols, "Circle").parents, ["Shape"]);
}

#[test]
fn an_accessor_macro_defines_methods_and_stays_a_call() {
    let raw = extract(SAMPLE);

    let width = symbol(&raw.symbols, "Geometry::Shape#width");
    assert_eq!(width.owner.as_deref(), Some("Geometry::Shape"));
    assert!(width.exported);
    assert!(raw.calls.iter().any(|call| call.callee == "attr_accessor"));
}

#[test]
fn attr_writer_defines_only_the_setter() {
    let raw = extract(b"\nclass Conf\n  attr_writer :level\nend\n");

    assert_eq!(names(&raw.symbols), ["Conf", "Conf#level="]);
}

#[test]
fn every_attribute_write_target_calls_the_setter_alone() {
    for (shape, source) in [
        ("plain", "def f\n  a.b = 1\nend\n"),
        ("safe navigation", "def f\n  a&.b = 1\nend\n"),
        ("colons", "def f\n  a::b = 1\nend\n"),
        ("list", "def f\n  c, a.b = 1, 2\nend\n"),
        ("nested list", "def f\n  c, (d, a.b) = 1, [2, 3]\nend\n"),
        ("splat", "def f\n  c, *a.b = 1, 2\nend\n"),
        ("rescue", "def f\n  1\nrescue => a.b\n  2\nend\n"),
        ("for", "def f\n  for a.b in [] do end\nend\n"),
    ] {
        let raw = extract(source.as_bytes());

        assert_eq!(sites_of(&raw, "b="), [("f", Some("a"))], "{shape}");
        assert_eq!(sites_of(&raw, "b"), [], "{shape}");
    }
}

#[test]
fn a_constant_attribute_write_calls_the_setter_of_the_constant_name() {
    let raw = extract(b"def f\n  a.B = 1\nend\n");

    assert_eq!(sites_of(&raw, "B="), [("f", Some("a"))]);
    assert_eq!(sites_of(&raw, "B"), []);
}

#[test]
fn an_operator_write_calls_the_reader_before_the_setter() {
    for operator in ["+=", "||=", "&&="] {
        let source = format!("def f\n  a.b {operator} 1\nend\n");
        let raw = extract(source.as_bytes());

        assert_eq!(
            calls(&raw),
            [("b", Some("a")), ("b=", Some("a")), ("a", None)],
            "{operator}"
        );
    }
}

#[test]
fn only_the_written_attribute_of_a_chain_is_a_setter() {
    let raw = extract(b"def f\n  self.b.c = d.e\nend\n");

    assert_eq!(
        calls(&raw),
        [
            ("c=", Some("")),
            ("b", Some("")),
            ("e", Some("d")),
            ("d", None)
        ]
    );
}

#[test]
fn a_call_is_made_by_the_method_around_it() {
    let raw = extract(SAMPLE);

    let compute = call(&raw.calls, "compute");
    assert_eq!(compute.site_of, id_of(&raw.symbols, "Geometry::Shape#area"));
    assert_eq!(compute.receiver, None);
    let draw = call(&raw.calls, "draw");
    assert_eq!(draw.site_of, id_of(&raw.symbols, "Circle#radius"));
    assert_eq!(draw.receiver.as_deref(), Some("helper"));
}

#[test]
fn a_call_before_the_first_symbol_is_a_call_from_file_scope() {
    let raw = extract(SAMPLE);

    assert!(
        raw.calls
            .iter()
            .any(|call| call.callee == "require" && call.site_of == FILE_SCOPE)
    );
}

#[test]
fn a_call_in_a_type_body_is_made_by_the_type_on_itself() {
    let raw = extract(
        b"require \"set\"
module Geo
  class Widget
    def self.track(name)
      name
    end

    track :size
    self.track :weight
    items.each { |item| register(item) }

    def read
      self.class.track(:read)
    end
  end
  helper(1)
end
boot(2)
",
    );
    let sites: Vec<(&str, &str, Option<&str>)> = raw
        .calls
        .iter()
        .map(|call| {
            let site_of = if call.site_of == FILE_SCOPE {
                ""
            } else {
                raw.symbols[call.site_of.0 as usize].qualified_name.as_str()
            };
            (site_of, call.callee.as_str(), call.receiver.as_deref())
        })
        .collect();

    assert_eq!(
        sites,
        [
            ("", "require", None),
            ("Geo::Widget", "track", Some("Geo::Widget.")),
            ("Geo::Widget", "track", Some("Geo::Widget.")),
            ("Geo::Widget", "each", Some("items")),
            ("Geo::Widget", "items", Some("Geo::Widget.")),
            ("Geo::Widget", "register", Some("Geo::Widget.")),
            ("Geo::Widget#read", "track", Some("::Geo::Widget.")),
            ("Geo::Widget#read", "class", Some("Geo::Widget")),
            ("Geo", "helper", Some("Geo.")),
            ("", "boot", None),
        ]
    );
}

#[test]
fn a_member_call_on_an_expression_keeps_the_empty_receiver() {
    let raw = extract(b"def f\n  items[0].close\n  @pool.get(1)\n  make(2).run(3)\nend\n");

    assert_eq!(
        calls(&raw),
        [
            ("close", Some("")),
            ("items", None),
            ("get", Some("")),
            ("run", Some("")),
            ("make", None),
        ]
    );
}

#[test]
fn a_self_receiver_names_the_type_chain_and_a_receiver_on_a_class_ends_in_the_class_method_separator()
 {
    let raw = extract(
        b"module Geo\n  class Shape\n    def area\n      1\n    end\n    def report\n      \
          self.area\n      Shape.build\n      ::Geo::Shape.build\n      self.class.build\n      \
          item.class.build\n    end\n    def self.make\n      self.build\n      self.class.name\n    end\n    \
          class << self\n      def remake\n        self.build\n      end\n    end\n  end\nend\n\
          def obj.tick\n  self.build\nend\n",
    );

    assert_eq!(
        calls(&raw),
        [
            ("area", Some("Geo::Shape")),
            ("build", Some("Shape.")),
            ("build", Some("::Geo::Shape.")),
            ("build", Some("::Geo::Shape.")),
            ("class", Some("Geo::Shape")),
            ("build", Some("")),
            ("class", Some("item")),
            ("item", None),
            ("build", Some("Geo::Shape.")),
            ("name", Some("")),
            ("class", Some("Geo::Shape.")),
            ("build", Some("Geo::Shape.")),
            ("build", Some("")),
        ]
    );
}

#[test]
fn a_literal_receiver_names_the_class_of_the_value_it_makes() {
    let raw = extract(
        b"def f\n  \"a\".freeze\n  'b'.size\n  <<~EOS.strip\n    x\n  EOS\n  `ls`.lines\n  ?c.ord\n  \
          [1].first\n  %w[a].join\n  %i[a].size\n  {a: 1}.keys\n  1.times\n  1.5.round\n  2r.to_f\n  3i.real\n  \
          :sym.to_proc\n  :\"sym\".size\n  /x/.match?(s)\n  -> { 1 }.call\n  nil.to_a\n  true.to_s\n  false.to_s\n  \
          (1..2).to_a\n  (1; 2).to_s\nend\n",
    );

    assert_eq!(
        calls(&raw)
            .into_iter()
            .filter(|(callee, _)| *callee != "s")
            .collect::<Vec<_>>(),
        [
            ("freeze", Some("String")),
            ("size", Some("String")),
            ("strip", Some("String")),
            ("lines", Some("String")),
            ("ord", Some("String")),
            ("first", Some("Array")),
            ("join", Some("Array")),
            ("size", Some("Array")),
            ("keys", Some("Hash")),
            ("times", Some("Integer")),
            ("round", Some("Float")),
            ("to_f", Some("Rational")),
            ("real", Some("Complex")),
            ("to_proc", Some("Symbol")),
            ("size", Some("Symbol")),
            ("match?", Some("Regexp")),
            ("call", Some("Proc")),
            ("to_a", Some("NilClass")),
            ("to_s", Some("TrueClass")),
            ("to_s", Some("FalseClass")),
            ("to_a", Some("Range")),
            ("to_s", Some("")),
        ]
    );
}

#[test]
fn a_file_under_lib_belongs_to_the_library_its_outermost_lib_roots() {
    for (path, library) in [
        ("lib/geo/shape.rb", Some("")),
        (
            "vendor/ruby/3.4/lib/ruby/3.4.0/set.rb",
            Some("vendor/ruby/3.4"),
        ),
        (
            "vendor/ruby/3.4/lib/ruby/gems/3.4.0/gems/rake-13.3.1/lib/rake.rb",
            Some("vendor/ruby/3.4"),
        ),
        ("gems/rake/lib/rake/task.rb", Some("gems/rake")),
        ("Library/Homebrew/formula.rb", None),
        ("vendor/library/x.rb", None),
    ] {
        assert_eq!(
            resolve(WIDGET, path, &[]).package.as_deref(),
            library,
            "{path}"
        );
    }
}

#[test]
fn a_file_under_no_lib_belongs_to_the_library_above_its_outermost_source_root() {
    for (path, library) in [
        ("spec/models/user_spec.rb", Some("")),
        ("test/test_helper.rb", Some("")),
        ("app/models/user.rb", Some("")),
        (
            "railties/test/application/routing_test.rb",
            Some("railties"),
        ),
        ("railties/test/app/routing_test.rb", Some("railties")),
        ("activestorage/app/models/blob.rb", Some("activestorage")),
        ("gems/rake/spec/task_spec.rb", Some("gems/rake")),
        ("gems/rake/test/lib/helper.rb", Some("gems/rake/test")),
        ("contest/foo.rb", None),
        ("guides/rails_guides/markdown.rb", None),
    ] {
        assert_eq!(
            resolve(WIDGET, path, &[]).package.as_deref(),
            library,
            "{path}"
        );
    }
}

#[test]
fn a_bare_new_in_a_class_method_names_an_instance_of_the_owner_and_elsewhere_is_as_written() {
    let raw = extract(
        b"module Geo\n  class Shape\n    TOP = new.freeze\n    def self.parse\n      new.getopts\n    end\n    \
          class << self\n      def make\n        new.build\n      end\n    end\n    def self.wrap(new)\n      \
          new.getopts\n    end\n    def self.bound\n      new = 1\n      \
          new.getopts\n    end\n    def self.captured\n      \
          new = 1\n      tap { new.getopts }\n    end\n    def report\n      \
          new.getopts\n    end\n  end\nend\n",
    );

    let receivers: Vec<_> = calls(&raw)
        .into_iter()
        .filter(|(callee, _)| *callee != "new")
        .collect();
    assert_eq!(
        receivers,
        [
            ("freeze", Some("new")),
            ("getopts", Some("Geo::Shape")),
            ("build", Some("Geo::Shape")),
            ("getopts", Some("new")),
            ("getopts", Some("new")),
            ("tap", None),
            ("getopts", Some("new")),
            ("getopts", Some("new")),
        ]
    );
}

#[test]
fn every_block_form_of_eval_and_exec_runs_on_a_receiver_the_constant_names() {
    for name in [
        "class_eval",
        "module_eval",
        "instance_eval",
        "class_exec",
        "module_exec",
        "instance_exec",
    ] {
        let source = format!(
            "class Shape\n  def self.parse\n    Geo::Other.{name} do\n      helper\n      self.reach\n      \
             new.getopts\n    end\n    after\n  end\nend\n"
        );
        let raw = extract(source.as_bytes());

        assert_eq!(
            calls(&raw)
                .into_iter()
                .filter(|(callee, _)| *callee != "new" && *callee != name)
                .collect::<Vec<_>>(),
            [
                ("helper", Some("Geo::Other.")),
                ("reach", Some("Geo::Other.")),
                ("getopts", Some("Geo::Other")),
                ("after", None),
            ],
            "{name}"
        );
    }
}

#[test]
fn a_block_on_a_receiver_the_walk_cannot_name_records_an_empty_receiver() {
    let raw = extract(
        b"class Shape\n  def self.parse(klass)\n    klass.class_eval do\n      helper\n      self.reach\n      \
          new.getopts\n      self.class.name\n    end\n    @obj.instance_eval { late }\n  end\nend\n",
    );

    assert_eq!(
        calls(&raw)
            .into_iter()
            .filter(|(callee, _)| !matches!(
                *callee,
                "new" | "class" | "class_eval" | "instance_eval"
            ))
            .collect::<Vec<_>>(),
        [
            ("helper", Some("")),
            ("reach", Some("")),
            ("getopts", Some("")),
            ("name", Some("")),
            ("late", Some("")),
        ]
    );
}

#[test]
fn a_block_on_a_literal_runs_on_a_value_of_the_literal_s_class() {
    let raw =
        extract(b"class Shape\n  def run\n    \"hello\".instance_exec { reverse }\n  end\nend\n");

    assert_eq!(sites_of(&raw, "reverse"), [("Shape#run", Some("String"))]);
}

#[test]
fn a_block_keeps_the_self_around_it_unless_eval_or_exec_runs_it_on_another_receiver() {
    let raw = extract(
        b"class Shape\n  class_eval { own }\n  self.instance_eval { own }\n  Other.tap { own }\n  \
          Other.instance_exec(own) { }\n  Other.class_eval(&own)\n  Other.class_eval(\"own\")\n  \
          class_eval(Other) { own }\nend\n",
    );

    assert_eq!(
        sites_of(&raw, "own"),
        [("Shape", Some("Shape.")); 6],
        "a call with no receiver in the body records the type"
    );
}

#[test]
fn a_method_or_a_type_inside_the_block_opens_its_own_self() {
    let raw = extract(
        b"class Shape\n  Other.class_eval do\n    def build\n      inner\n    end\n    class Nested\n      \
          nested\n    end\n    after\n  end\nend\n",
    );

    assert_eq!(sites_of(&raw, "inner")[0].1, None);
    assert_eq!(sites_of(&raw, "nested")[0].1, Some("Shape::Nested."));
    assert_eq!(sites_of(&raw, "after")[0], ("Shape", Some("Other.")));
}

#[test]
fn the_innermost_of_nested_blocks_names_self_and_a_block_of_own_self_keeps_it() {
    let raw = extract(
        b"def run\n  Other.class_eval do\n    Third.instance_eval { first }\n    \
          self.class_eval { second }\n    class_eval { third }\n    fourth\n  end\nend\n",
    );

    assert_eq!(sites_of(&raw, "first")[0].1, Some("Third."));
    for name in ["second", "third", "fourth"] {
        assert_eq!(sites_of(&raw, name)[0].1, Some("Other."), "{name}");
    }
}

#[test]
fn an_argument_of_the_inner_call_runs_on_the_receiver_of_the_outer_block() {
    let raw = extract(b"def run\n  Other.class_eval do\n    Third.instance_exec(argument) { inner }\n  end\nend\n");

    assert_eq!(sites_of(&raw, "argument")[0].1, Some("Other."));
    assert_eq!(sites_of(&raw, "inner")[0].1, Some("Third."));
}

#[test]
fn self_class_in_the_block_is_no_class_of_an_instance_of_the_owner() {
    let raw = extract(
        b"class Shape\n  def run\n    Other.class_eval { self.class.name }\n    self.class.outer\n  end\nend\n",
    );

    assert_eq!(sites_of(&raw, "name")[0].1, Some(""));
    assert_eq!(sites_of(&raw, "outer")[0].1, Some("::Shape."));
}

#[test]
fn self_class_in_a_module_s_instance_method_names_no_class() {
    let raw = extract(
        b"module Cached\n  def reload\n    self.class.fetch\n  end\n  class Entry\n    def load\n      \
          self.class.parse\n    end\n  end\nend\n",
    );

    assert_eq!(sites_of(&raw, "fetch")[0].1, Some("."));
    assert_eq!(sites_of(&raw, "parse")[0].1, Some("::Cached::Entry."));
}

#[test]
fn self_class_in_a_class_written_as_a_path_names_no_class() {
    let raw = extract(
        b"class Gem::Cargo < Gem::Builder\n  def flags\n    self.class.split\n  end\nend\n",
    );

    assert_eq!(sites_of(&raw, "split")[0].1, Some("."));
}

#[test]
fn a_bound_new_in_the_block_stays_a_local() {
    let raw = extract(b"def run(new)\n  Other.class_eval { new.getopts }\nend\n");

    assert_eq!(sites_of(&raw, "getopts")[0].1, Some("new"));
}

#[test]
fn a_use_through_a_symbol_in_the_block_is_of_its_receiver() {
    let raw = extract(
        b"def run(klass)\n  Other.class_eval { send(:named) }\n  klass.class_eval { send(:unnamed) }\nend\n",
    );

    let used: Vec<_> = uses(&raw)
        .into_iter()
        .map(|(name, receiver, _, _)| (name, receiver))
        .collect();
    assert_eq!(used, [("named", &Receiver::Path("Other.".to_owned()))]);
}

fn sites_of<'r>(raw: &'r RawExtraction, callee: &str) -> Vec<(&'r str, Option<&'r str>)> {
    raw.calls
        .iter()
        .filter(|call| call.callee == callee)
        .map(|call| {
            let site_of = if call.site_of == FILE_SCOPE {
                ""
            } else {
                raw.symbols[call.site_of.0 as usize].qualified_name.as_str()
            };
            (site_of, call.receiver.as_deref())
        })
        .collect()
}

#[test]
fn a_let_records_the_shared_group_every_file_reaches_and_a_group_names_what_it_includes() {
    let raw = extract(
        b"RSpec.shared_context 'text' do
  let(:from_text) { 1 }
end

shared_examples :symbol do
  let(:from_symbol) { 2 }
end

helper.shared_context 'on a value' do
  let(:from_a_value) { 4 }
end

RSpec.describe 'a' do
  shared_context 'scoped' do
    let(:from_scoped) { 3 }
  end

  include_context 'text'
  include_examples :symbol

  context 'b' do
    include_context \"inner\"

    def helper
      read
    end
  end

  it 'reads' do
    read
  end
end
",
    );

    let groups = |name| symbol(&raw.symbols, name).value_types.as_slice();
    assert_eq!(
        (
            groups("from_text"),
            groups("from_symbol"),
            groups("from_a_value"),
            groups("from_scoped"),
        ),
        (
            &["'text'".to_owned()][..],
            &[":symbol".to_owned()][..],
            &[][..],
            &[][..]
        )
    );
    let read = raw.calls.iter().find(|call| call.callee == "read");
    assert_eq!(
        read.and_then(|call| call.receiver.as_deref()),
        Some("+'inner'\n:symbol\n'text'")
    );
    let uses: Vec<&str> = raw.uses.iter().map(|site| site.name.as_str()).collect();
    assert_eq!(uses, [":symbol#read", "'text'#read"]);
}

#[test]
fn a_filter_of_a_block_marks_its_reads_and_a_config_records_what_each_filter_includes() {
    let raw = extract(
        b"RSpec.configure do |config|
  config.include_context 'text', :first, :second
  config.include_context 'for every group'
end

RSpec.describe 'a', :first, type: :system do
  include_context 'nearer'

  it 'reads', :second do
    def helper
      read
    end

    at_top
  end
end

Other.describe 'b', :third do
  def beside
    read_beside
  end
end
",
    );

    let read = raw.calls.iter().find(|call| call.callee == "read");
    assert_eq!(
        read.and_then(|call| call.receiver.as_deref()),
        Some("+@second\n'nearer'\n@first")
    );
    let beside = raw.calls.iter().find(|call| call.callee == "read_beside");
    assert_eq!(beside.and_then(|call| call.receiver.as_deref()), None);
    let uses: Vec<&str> = raw.uses.iter().map(|site| site.name.as_str()).collect();
    assert_eq!(
        uses,
        [
            "'text'@first",
            "'text'@second",
            "'text'",
            "@second#at_top",
            "'nearer'#at_top",
            "@first#at_top"
        ]
    );
}

#[test]
fn a_block_that_runs_on_a_receiver_keeps_it_inside_a_group_that_includes_a_context() {
    let raw = extract(
        b"RSpec.describe 'a' do
  include_context 'text'

  def helper
    Other.class_eval do
      read
    end
  end
end
",
    );

    let read = raw.calls.iter().find(|call| call.callee == "read");
    assert_eq!(
        read.and_then(|call| call.receiver.as_deref()),
        Some("Other.")
    );
}

#[test]
fn a_let_or_a_subject_with_a_symbol_and_a_block_defines_a_method_local_to_the_file() {
    let raw = extract(
        b"RSpec.describe 'a' do
  let(:one) { 1 }
  let!(:two) { 2 }
  subject(:three) { 3 }
  subject!(:four) do
    4
  end
  let(:no_block)
  let('text') { 5 }
  subject { 6 }
  helper.let(:on_a_value) { 7 }
end
",
    );

    assert_eq!(names(&raw.symbols), ["one", "two", "three", "four"]);
    assert_eq!(
        raw.local_symbols,
        [SymbolId(0), SymbolId(1), SymbolId(2), SymbolId(3)]
    );
    let four = symbol(&raw.symbols, "four");
    assert_eq!(
        (
            four.kind,
            four.signature.as_deref(),
            four.line,
            four.end_line
        ),
        (SymbolKind::Function, Some("subject!(:four)"), 5, 7)
    );
}

#[test]
fn a_group_on_one_line_holds_only_the_calls_inside_it() {
    let raw = extract(
        b"RSpec.describe('a') { let(:item) { 1 }; context('b') { let(:item) { 2 }; def inner; item; end }; \
          context('c') { let(:other) { 3 }; def sibling; other; end }; context('d') { def outsider; other; end }; \
          def outer; item; end }\n",
    );

    let bound: Vec<(u32, u32)> = raw
        .local_calls
        .iter()
        .map(|call| (call.site_of.0, call.local.0))
        .collect();
    assert_eq!(
        names(&raw.symbols),
        [
            "item", "item", "inner", "other", "sibling", "outsider", "outer"
        ]
    );
    assert_eq!(bound, [(2, 1), (4, 3), (6, 0)]);
    assert_eq!(calls(&raw).last(), Some(&("other", None)));
}

#[test]
fn a_bare_word_is_a_call_unless_a_binding_before_it_makes_it_a_local() {
    let raw = extract(
        b"class Task
  def run
    1
  end

  def go
    run
  end

  def go_parens
    run()
  end

  def go_self
    self.run
  end

  def shadowed
    run = 2
    run
  end
end
",
    );

    assert_eq!(
        sites_of(&raw, "run"),
        [
            ("Task#go", None),
            ("Task#go_parens", None),
            ("Task#go_self", Some("Task")),
        ]
    );
}

#[test]
fn every_binding_form_makes_a_bare_word_a_local() {
    let forms: [(&str, &[u8]); 21] = [
        ("parameter", b"def f(run)\n  run\nend\n"),
        ("optional parameter", b"def f(run = 1)\n  run\nend\n"),
        ("splat parameter", b"def f(*run)\n  run\nend\n"),
        ("keyword parameter", b"def f(run:)\n  run\nend\n"),
        ("double splat parameter", b"def f(**run)\n  run\nend\n"),
        ("block parameter", b"def f(&run)\n  run\nend\n"),
        ("destructured parameter", b"def f((a, run))\n  run\nend\n"),
        ("block's parameter", b"items.each { |run| run }\n"),
        (
            "block-local name",
            b"items.each { |x; run| run = x\n run }\n",
        ),
        ("lambda parameter", b"->(run) { run }\n"),
        ("assignment", b"run = 1\nrun\n"),
        ("multiple assignment", b"a, (b, *run) = list\nrun\n"),
        ("operator assignment", b"run ||= 1\nrun\n"),
        ("for target", b"for run in list do run end\n"),
        (
            "rescue variable",
            b"begin\nrescue Error => run\n  run\nend\n",
        ),
        ("array pattern", b"case v\nin [a, run] then run\nend\n"),
        ("hash pattern key", b"case v\nin {run:} then run\nend\n"),
        ("hash pattern value", b"case v\nin {k: run} then run\nend\n"),
        ("as pattern", b"case v\nin Integer => run then run\nend\n"),
        ("rightward pattern", b"v => [run]\nrun\n"),
        ("named regexp group", b"/(?<run>a)/ =~ text\nrun\n"),
    ];

    let calling: Vec<&str> = forms
        .iter()
        .filter(|(_, source)| !sites_of(&extract(source), "run").is_empty())
        .map(|(form, _)| *form)
        .collect();
    assert_eq!(calling, [] as [&str; 0]);
}

#[test]
fn a_binding_reaches_the_rest_of_its_scope_and_the_blocks_inside_it() {
    let raw = extract(
        b"def before
  run
  run = 1
  items.each { run }
end

def other
  run
  it
end

def in_block
  items.each { run = 1 }
  run
  -> { it + _1 }
  items.each { it }
  __FILE__
end

target = 1
class << target
  target
end
",
    );

    assert_eq!(
        sites_of(&raw, "run"),
        [("before", None), ("other", None), ("in_block", None)]
    );
    assert_eq!(
        sites_of(&raw, "items"),
        [("before", None), ("in_block", None), ("in_block", None)]
    );
    assert_eq!(sites_of(&raw, "it"), [("other", None)]);
    assert_eq!(sites_of(&raw, "_1"), []);
    assert_eq!(sites_of(&raw, "__FILE__"), []);
    assert_eq!(sites_of(&raw, "target"), [("", None)]);
}

#[test]
fn a_bare_word_in_a_type_body_is_a_call_on_the_type_and_a_modifier_still_sets_visibility() {
    let raw = extract(b"class Conf\n  setup\n  private\n  def hidden\n  end\nend\n");

    assert_eq!(sites_of(&raw, "setup"), [("Conf", Some("Conf."))]);
    assert_eq!(sites_of(&raw, "private"), [("Conf", Some("Conf."))]);
    assert!(!symbol(&raw.symbols, "Conf#hidden").exported);
}

#[test]
fn an_escaped_or_bracketed_parenthesis_opens_no_named_group() {
    for source in [
        &b"/\\(?<run>a)/ =~ text\nrun\n"[..],
        b"/[(?<run>]a/ =~ text\nrun\n",
    ] {
        let raw = extract(source);
        assert_eq!(sites_of(&raw, "run"), [("", None)]);
    }
}

#[test]
fn a_name_in_a_definition_or_a_pin_is_no_call() {
    let raw = extract(
        b"alias copy run\nundef run\ndef run\nend\ndef obj.run\nend\ncase v\nin ^run then 1\nend\n",
    );

    assert_eq!(sites_of(&raw, "run"), []);
    assert_eq!(sites_of(&raw, "copy"), []);
    assert_eq!(sites_of(&raw, "obj"), []);
}

#[test]
fn an_endless_method_a_builder_and_a_singleton_object() {
    let raw = extract(
        b"\ndef area = 3.14\n\nPoint = Struct.new(:x, :y)\nRecord = Data.define(:a, :b)\nLIMIT = 10\n\n\
          def Config.load\nend\n\nobj = Object.new\ndef obj.greet\nend\n",
    );

    assert_eq!(
        symbol(&raw.symbols, "area").signature.as_deref(),
        Some("def area")
    );
    assert_eq!(symbol(&raw.symbols, "Point").kind, SymbolKind::Class);
    assert_eq!(symbol(&raw.symbols, "Record").kind, SymbolKind::Class);
    assert_eq!(symbol(&raw.symbols, "LIMIT").kind, SymbolKind::Constant);
    assert_eq!(
        symbol(&raw.symbols, "Config.load").owner.as_deref(),
        Some("Config")
    );
    assert_eq!(symbol(&raw.symbols, "greet").owner, None);
}

#[test]
fn a_write_to_an_identifier_or_a_global_defines_nothing() {
    let raw = extract(b"limit = 3\n$debug = true\n");

    assert_eq!(raw.symbols.len(), 0);
}

#[test]
fn a_scoped_constant_write_defines_the_constant_on_the_module_its_path_names() {
    let raw = extract(
        b"Foo::BAR = 5\nA::B::C = 6\n::Rooted = 7\n::A::Deep = 8\nmodule Geo\n  module Shape\n    Geo::MAX = 9\n    \
          Shape::Edge::MIN = 10\n    Other::LIMIT = 11\n    ::Top = 12\n  end\nend\n",
    );

    let qualified: Vec<(&str, &str, Option<&str>)> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Constant)
        .map(|symbol| {
            (
                symbol.name.as_str(),
                symbol.qualified_name.as_str(),
                symbol.owner.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        qualified,
        [
            ("BAR", "Foo::BAR", Some("Foo")),
            ("C", "A::B::C", Some("A::B")),
            ("Rooted", "Rooted", None),
            ("Deep", "A::Deep", Some("A")),
            ("MAX", "Geo::MAX", Some("Geo")),
            ("MIN", "Geo::Shape::Edge::MIN", Some("Geo::Shape::Edge")),
            ("LIMIT", "Other::LIMIT", Some("Other")),
            ("Top", "Top", None),
        ]
    );
}

#[test]
fn a_scoped_constant_write_on_a_path_the_source_does_not_spell_defines_nothing() {
    let raw = extract(b"a::B = 1\nself::C = 2\nfoo.bar::D = 3\nA.b::E = 4\n");

    assert_eq!(raw.symbols.len(), 0);
}

#[test]
fn a_scoped_constant_bound_to_a_struct_is_a_class() {
    let raw = extract(b"Geo::Point = Struct.new(:x, :y)\n");

    assert_eq!(symbol(&raw.symbols, "Geo::Point").kind, SymbolKind::Class);
}

#[test]
fn a_scoped_constant_records_the_class_its_value_holds() {
    let raw = extract(
        b"A::Made = Foo.new(1)\nA::Same = Foo\nA::Deep = Foo::Bar\nA::Rooted = ::Foo::Bar.new\nA::Lit = 5\n\
          A::Call = compute\nA::Cls = Class.new(Foo)\nA::Mod = Module.new\nA::Other = Foo.build\n\
          module Geo\n  Geo::Near = Geo::Shape.new\n  Geo::Same = Geo\nend\nPlain = Foo.new\n",
    );

    let held = |name: &str| symbol(&raw.symbols, name).value_types.clone();
    assert_eq!(held("A::Made"), ["Foo"]);
    assert_eq!(held("A::Same"), ["Foo."]);
    assert_eq!(held("A::Deep"), ["Foo::Bar."]);
    assert_eq!(held("A::Rooted"), ["Foo::Bar"]);
    assert_eq!(held("Geo::Near"), ["Geo::Shape"]);
    assert_eq!(held("Geo::Same"), ["Geo."]);
    for untold in ["A::Lit", "A::Call", "A::Cls", "A::Mod", "A::Other"] {
        assert_eq!(held(untold), [""], "{untold}");
    }
    assert_eq!(held("Plain"), Vec::<String>::new());
}

#[test]
fn a_signature_is_the_header_without_the_body() {
    let raw = extract(b"def greet(name)\n  puts name\nend\n\ndef self.build(x)\n  make x\nend\n");

    assert_eq!(
        symbol(&raw.symbols, "greet").signature.as_deref(),
        Some("def greet(name)")
    );
    assert_eq!(
        symbol(&raw.symbols, "build").signature.as_deref(),
        Some("def self.build(x)")
    );
}

#[test]
fn a_deep_type_takes_every_ancestor_into_its_chain() {
    let raw = extract(b"\nclass A\n  class B\n    class C\n    end\n  end\nend\n");

    assert_eq!(
        symbol(&raw.symbols, "A::B::C").owner.as_deref(),
        Some("A::B")
    );
}

#[test]
fn a_type_inside_a_method_has_no_owner() {
    let raw = extract(
        b"\nmodule Geometry\n  class Outer\n    def m\n      class Inner\n      end\n    end\n  end\nend\n",
    );

    assert_eq!(symbol(&raw.symbols, "Inner").owner, None);
}

#[test]
fn every_member_form_takes_the_chain_of_its_type() {
    let raw = extract(
        br"
module A
  module B
    class C
      attr_reader :size
      LIMIT = 1
      def i; end
      def self.s; end
      def C.k; end
      def B.m; end
      def Other.o; end
      class << self
        def t; end
        alias u t
        alias_method :v, :t
      end
      alias j i
      alias_method :l, :i
    end
  end
end
",
    );

    let owners: Vec<(&str, Option<&str>)> = raw
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.owner.as_deref()))
        .collect();
    assert_eq!(
        owners,
        [
            ("A", None),
            ("A::B", Some("A")),
            ("A::B::C", Some("A::B")),
            ("A::B::C#size", Some("A::B::C")),
            ("A::B::C::LIMIT", Some("A::B::C")),
            ("A::B::C#i", Some("A::B::C")),
            ("A::B::C.s", Some("A::B::C")),
            ("A::B::C.k", Some("A::B::C")),
            ("A::B.m", Some("A::B")),
            ("Other.o", Some("Other")),
            ("A::B::C.t", Some("A::B::C")),
            ("A::B::C.u", Some("A::B::C")),
            ("A::B::C.v", Some("A::B::C")),
            ("A::B::C#j", Some("A::B::C")),
            ("A::B::C#l", Some("A::B::C")),
        ]
    );
}

#[test]
fn two_types_of_one_name_keep_their_members_apart() {
    let raw = extract(
        b"\nmodule A\n  class Worker\n    def run; end\n    alias go run\n  end\nend\n\
          module B\n  class Worker\n    def run; end\n    alias go run\n  end\nend\n",
    );

    let methods: Vec<&str> = raw
        .symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Method)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert_eq!(
        methods,
        [
            "A::Worker#run",
            "A::Worker#go",
            "B::Worker#run",
            "B::Worker#go"
        ]
    );
}

#[test]
fn a_mixin_is_a_parent_of_the_type_whose_body_holds_it() {
    let raw = extract(
        b"\nmodule A\n  class Worker\n    class Worker\n    end\n    include Walkable\n  end\nend\n",
    );

    assert_eq!(symbol(&raw.symbols, "A::Worker").parents, ["Walkable"]);
    assert_eq!(symbol(&raw.symbols, "A::Worker::Worker").parents.len(), 0);
}

#[test]
fn a_macro_with_a_receiver_adds_no_parent() {
    let raw = extract(
        b"\nclass Shape\n  include Walkable\n  Other.include Comparable\n  Other.new.extend Comparable\nend\n",
    );

    assert_eq!(symbol(&raw.symbols, "Shape").parents, ["Walkable"]);
}

#[test]
fn visibility_modifiers_decide_what_is_exported() {
    let raw = extract(
        br"
class Vault
  def opened
    1
  end

  def _helper
    9
  end

  private

  def hidden
    2
  end

  def _looks_private
    3
  end

  public

  def reopened
    4
  end

  protected

  def shared
    5
  end

  def redacted
    6
  end
  private :redacted

  private def inlined
    7
  end

  private

  public def forced_public
    8
  end

  protected def kept_visible
    9
  end
end
",
    );

    let exported = |qualified: &str| symbol(&raw.symbols, qualified).exported;
    assert!(exported("Vault#opened"));
    assert!(exported("Vault#_helper"));
    assert!(!exported("Vault#hidden"));
    assert!(!exported("Vault#_looks_private"));
    assert!(exported("Vault#reopened"));
    assert!(exported("Vault#shared"));
    assert!(!exported("Vault#redacted"));
    assert!(!exported("Vault#inlined"));
    assert!(exported("Vault#forced_public"));
    assert!(exported("Vault#kept_visible"));
}

#[test]
fn a_private_method_after_the_sample_modifier_is_unexported() {
    let raw = extract(SAMPLE);

    assert!(!symbol(&raw.symbols, "Geometry::Shape#_secret").exported);
}

#[test]
fn a_visibility_argument_finds_the_method_by_its_chain() {
    let raw = extract(
        b"\nmodule A\n  class Vault\n    def redacted; end\n    private :redacted\n  end\nend\n",
    );

    assert!(!symbol(&raw.symbols, "A::Vault#redacted").exported);
}

#[test]
fn a_modifier_inside_a_method_sets_nothing() {
    let raw =
        extract(b"\nclass Vault\n  def setup\n    private\n  end\n\n  def after\n  end\nend\n");

    assert!(symbol(&raw.symbols, "Vault#after").exported);
}

#[test]
fn accessors_take_the_running_visibility() {
    let raw = extract(
        b"\nclass Vault\n  attr_reader :opened\n\n  private\n\n  attr_accessor :secret\n  attr_writer :sealed\nend\n",
    );

    let exported = |qualified: &str| symbol(&raw.symbols, qualified).exported;
    assert!(exported("Vault#opened"));
    assert!(!exported("Vault#secret"));
    assert!(!exported("Vault#secret="));
    assert!(!exported("Vault#sealed="));
}

#[test]
fn a_def_inside_class_self_is_a_class_method() {
    let raw = extract(
        b"\nclass Conf\n  class << self\n    def load\n      1\n    end\n  end\n\n  def fetch\n    2\n  end\nend\n",
    );

    assert!(symbol(&raw.symbols, "Conf.load").exported);
    assert_eq!(symbol(&raw.symbols, "Conf#fetch").kind, SymbolKind::Method);
}

#[test]
fn a_modifier_inside_class_self_covers_the_class_methods_after_it() {
    let raw = extract(
        br"
class Conf
  class << self
    def open; end
    def named; end
    private :named
    private
    def secret; end
    attr_reader :hidden
    alias covert open
  end

  def named; end
end
",
    );

    let exported = |qualified: &str| symbol(&raw.symbols, qualified).exported;
    assert!(exported("Conf.open"));
    assert!(!exported("Conf.named"));
    assert!(!exported("Conf.secret"));
    assert!(!exported("Conf.hidden"));
    assert!(!exported("Conf.covert"));
    assert!(exported("Conf#named"));
}

#[test]
fn private_class_method_hides_a_class_method_and_a_bare_private_does_not() {
    let raw = extract(
        b"\nclass Conf\n  def self.make; end\n  private_class_method :make\n\n  private\n\n  \
          def self.open; end\n  def hidden; end\nend\n",
    );

    let exported = |qualified: &str| symbol(&raw.symbols, qualified).exported;
    assert!(!exported("Conf.make"));
    assert!(exported("Conf.open"));
    assert!(!exported("Conf#hidden"));
}

#[test]
fn a_class_method_modifier_around_a_def_self_sets_that_class_method() {
    let raw = extract(
        br"
class Job
  private_class_method def self.helper; end
  def self.named; end
  private_class_method :named
  public_class_method def self.run; end
  private_class_method def Other.elsewhere; end
  private_class_method def handle.detached; end
end
",
    );

    let exported = |qualified: &str| symbol(&raw.symbols, qualified).exported;
    assert!(!exported("Job.helper"));
    assert!(!exported("Job.named"));
    assert!(exported("Job.run"));
    assert!(exported("detached"));
    assert!(exported("Other.elsewhere"));
}

#[test]
fn an_alias_defines_the_new_name_as_one_callable_with_its_original() {
    let resolved = resolve(
        b"\nclass Api\n  def fetch\n    1\n  end\n\n  alias get fetch\n  alias_method :retrieve, :fetch\n\n  \
          private\n\n  def secret\n    2\n  end\n\n  alias hidden secret\nend\n",
        "api.rb",
        &[],
    );

    let get = symbol(&resolved.symbols, "Api#get");
    assert_eq!(get.kind, SymbolKind::Method);
    assert_eq!(get.owner.as_deref(), Some("Api"));
    assert!(get.exported);
    assert_eq!(
        symbol(&resolved.symbols, "Api#retrieve").owner.as_deref(),
        Some("Api")
    );
    assert!(!symbol(&resolved.symbols, "Api#hidden").exported);

    let record = |qualified: &str, body: &str, role| {
        (
            id_of(&resolved.symbols, qualified),
            format!("api.rb#{}", id_of(&resolved.symbols, body).0),
            role,
        )
    };
    let records: Vec<(SymbolId, String, CallableRole)> = resolved
        .callables
        .iter()
        .map(|callable| (callable.symbol, callable.key.clone(), callable.role))
        .collect();
    assert_eq!(
        records,
        [
            record("Api#fetch", "Api#fetch", CallableRole::Definition),
            record("Api#get", "Api#fetch", CallableRole::Declaration),
            record("Api#retrieve", "Api#fetch", CallableRole::Declaration),
            record("Api#secret", "Api#secret", CallableRole::Definition),
            record("Api#hidden", "Api#secret", CallableRole::Declaration),
        ]
    );
}

#[test]
fn an_alias_keeps_the_body_its_original_named_at_the_alias() {
    let raw = extract(
        b"\nclass Api\n  def fetch\n    1\n  end\n  alias get fetch\n  def fetch\n    2\n  end\nend\n",
    );

    let first = raw.symbols.iter().position(|symbol| symbol.name == "fetch");
    let first = SymbolId(u32::try_from(first.unwrap_or(usize::MAX)).unwrap_or(u32::MAX));
    let keys: Vec<&str> = raw
        .callables
        .iter()
        .map(|callable| callable.key.as_str())
        .collect();
    assert_eq!(keys, [first.0.to_string(), first.0.to_string()]);
    assert_eq!(raw.callables[0].symbol, first);
}

#[test]
fn an_alias_its_own_name_redefines_is_no_callable() {
    let raw =
        extract(b"\nclass Api\n  def fetch\n  end\n  alias get fetch\n  def get\n  end\nend\n");

    assert_eq!(raw.callables.len(), 0);
}

#[test]
fn an_alias_of_an_undeclared_original_is_its_own_callable() {
    let raw = extract(b"\nclass Api < Base\n  alias get fetch\nend\n");

    assert_eq!(symbol(&raw.symbols, "Api#get").kind, SymbolKind::Method);
    assert_eq!(raw.callables.len(), 0);
}

#[test]
fn a_def_inside_a_method_binds_apart_from_a_top_level_one() {
    let raw = extract(
        b"def fetch; end\ndef setup\n  def fetch; end\n  alias inner fetch\nend\nalias outer fetch\n",
    );

    let key_of = |alias: &str| {
        raw.callables
            .iter()
            .find(|callable| callable.symbol == id_of(&raw.symbols, alias))
            .map_or_else(
                || panic!("no callable of {alias}"),
                |callable| callable.key.clone(),
            )
    };
    let fetches: Vec<String> = raw
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, symbol)| symbol.name == "fetch")
        .map(|(at, _)| at.to_string())
        .collect();
    assert_eq!(key_of("inner"), fetches[1]);
    assert_eq!(key_of("outer"), fetches[0]);
}

#[test]
fn an_alias_inside_class_self_is_a_class_method() {
    let raw = extract(
        b"\nclass Vault\n  def open\n  end\n  alias unlock open\n\n  class << self\n    def reset\n    end\n    \
          alias clear reset\n    alias_method :wipe, :reset\n  end\nend\n",
    );

    assert_eq!(
        symbol(&raw.symbols, "Vault#unlock").kind,
        SymbolKind::Method
    );
    for qualified in ["Vault.clear", "Vault.wipe"] {
        let alias = symbol(&raw.symbols, qualified);
        assert_eq!(alias.owner.as_deref(), Some("Vault"), "{qualified}");
        assert!(alias.exported, "{qualified}");
    }
    assert!(
        !raw.symbols
            .iter()
            .any(|symbol| symbol.qualified_name == "Vault#clear")
    );
}

#[test]
fn only_a_bare_require_with_a_string_is_a_require() {
    let resolved = resolve(
        b"\nrequire 'real_dep'\n\nclass SomeConfig\n  def require(path)\n  end\nend\n\n\
          SomeConfig.new.require 'phantom'\nSomeConfig.require 'phantom_too'\nrequire File.join(\"a\", \"b\")\n",
        "cfg.rb",
        &[],
    );

    assert_eq!(imports(&resolved), ["real_dep"]);
}

#[test]
fn an_interpolated_or_escaped_require_names_nothing() {
    let resolved = resolve(
        b"\nrequire \"cmd/#{name}\"\nrequire \"\\x61\"\nrequire 'plain'\n\nclass Main\nend\n",
        "main.rb",
        &["a.rb", "main.rb"],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert_eq!(imports(&resolved), ["plain"]);
}

#[test]
fn a_require_of_a_repository_file_is_an_include_and_no_import() {
    let resolved = resolve(
        b"\nrequire \"set\"\nrequire_relative 'helper'\n\ndef go\n  1\nend\n",
        "main.rb",
        &["helper.rb", "main.rb"],
    );

    assert_eq!(includes(&resolved), ["helper.rb"]);
    assert_eq!(imports(&resolved), ["set"]);
}

#[test]
fn require_relative_resolves_from_the_requiring_file_with_or_without_its_extension() {
    let files = ["app/models/user.rb", "app/models/account.rb"];

    for spec in ["account", "account.rb"] {
        let source = format!("\nrequire_relative \"{spec}\"\n\nclass User\nend\n");
        let resolved = resolve(source.as_bytes(), "app/models/user.rb", &files);
        assert_eq!(includes(&resolved), ["app/models/account.rb"], "{spec}");
    }
}

#[test]
fn a_require_resolves_under_the_first_load_root_that_holds_it() {
    let resolved = resolve(
        b"\nrequire \"widget/painter\"\nrequire \"config/boot\"\n\nclass Main\nend\n",
        "main.rb",
        &[
            "lib/widget/painter.rb",
            "app/widget/painter.rb",
            "config/boot.rb",
            "main.rb",
        ],
    );

    assert_eq!(
        includes(&resolved),
        ["lib/widget/painter.rb", "config/boot.rb"]
    );
}

#[test]
fn a_require_of_a_gem_is_an_import_as_written() {
    let resolved = resolve(
        b"\nrequire \"json\"\nrequire \"active_support/core_ext\"\n\nclass Main\nend\n",
        "main.rb",
        &["lib/app.rb", "main.rb"],
    );

    assert_eq!(includes(&resolved), Vec::<&str>::new());
    assert_eq!(imports(&resolved), ["json", "active_support/core_ext"]);
}

#[test]
fn a_test_path_marks_test_code() {
    for path in [
        "foo_test.rb",
        "foo_spec.rb",
        "app/spec/util.rb",
        "lib/test/run.rb",
        "lib/tests/run.rb",
        "lib/Tests/run.rb",
    ] {
        let resolved = resolve(WIDGET, path, &[]);
        assert_eq!(resolved.symbols.len(), 2, "{path}");
        assert!(
            resolved.symbols.iter().all(|symbol| symbol.flags.is_test()),
            "{path}"
        );
    }

    let production = resolve(WIDGET, "lib/foo.rb", &[]);
    assert!(
        production
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_test())
    );
}

#[test]
fn a_protobuf_name_or_a_header_marker_marks_generated_code() {
    let generated = |source: &[u8], path: &str| {
        let resolved = resolve(source, path, &[]);
        assert_eq!(resolved.symbols.len(), 2, "{path}");
        resolved
            .symbols
            .iter()
            .all(|symbol| symbol.flags.is_generated())
    };

    assert!(generated(WIDGET, "proto/widget_pb.rb"));
    assert!(generated(
        concat!(
            "# frozen_string_literal: true\n# @generated by tooling - DO NOT EDIT\n\n",
            "class Widget\n  def run\n    1\n  end\nend\n",
        )
        .as_bytes(),
        "lib/widget.rb"
    ));
    assert!(generated(
        b"# This file is auto-generated\nclass Widget\n  def run\n    1\n  end\nend\n",
        "lib/widget.rb"
    ));
    assert!(!generated(
        b"# A plain widget.\nclass Widget\n  def run\n    1\n  end\nend\n",
        "lib/widget.rb"
    ));
    assert!(
        !generated(
            b"=begin\n@generated\n=end\nclass Widget\n  def run\n    1\n  end\nend\n",
            "lib/widget.rb"
        ),
        "a block comment is code to the banner"
    );
    assert!(!generated(
        b"\nclass Widget\n  # @generated\n  def run\n    1\n  end\nend\n",
        "lib/widget.rb"
    ));
}

#[test]
fn no_symbol_is_an_entry_point() {
    let script = resolve(b"\ndef main\n  run\nend\n\nmain\n", "bin/run.rb", &[]);

    assert_eq!(script.symbols.len(), 1);
    assert!(script.symbols.iter().all(|symbol| !symbol.flags.is_entry()));
    assert!(
        extract(SAMPLE)
            .symbols
            .iter()
            .all(|symbol| !symbol.flags.is_entry())
    );
}

#[test]
fn a_bare_call_reaches_its_own_type_first_and_admits_inherited_members() {
    let conventions = Ruby::new().conventions();

    assert_eq!(conventions.own_members, OwnMembers::First);
    assert_eq!(conventions.inherited_members, InheritedMembers::Admitted);
    assert_eq!(conventions.owner_chain_separator, Some("::"));
}

#[test]
fn a_file_is_claimed_by_its_extension_in_any_case() {
    let ruby = Ruby::new();

    assert!(ruby.claims("lib/a.rb"));
    assert!(ruby.claims("lib/A.RB"));
    assert!(!ruby.claims("Gemfile"));
    assert!(!ruby.claims("lib/a.rbs"));
    assert_eq!(ruby.name(), "ruby");
}

#[test]
fn the_fingerprint_names_the_crate_and_is_computed_once() {
    let (one, other) = (Ruby::new(), Ruby::new());

    assert!(one.fingerprint().starts_with(b"kartta-lang-ruby "));
    assert!(std::ptr::eq(one.fingerprint(), other.fingerprint()));
}

const DOCUMENTED: &str = "require 'json'

# Parses.
def parse
end

=begin
Renders.
#=end
=end
def render
end
";

fn doc_line(name: &str) -> u32 {
    let lines: Vec<&[u8]> = DOCUMENTED.lines().map(str::as_bytes).collect();
    let line = symbol(&extract(DOCUMENTED.as_bytes()).symbols, name).line;
    Ruby::new().doc_comments().first_line(&lines, line)
}

#[test]
fn hash_comments_and_an_embedded_document_above_a_method_document_it() {
    assert_eq!(doc_line("parse"), 3);
    assert_eq!(doc_line("render"), 7);
}

fn uses(raw: &RawExtraction) -> Vec<(&str, &Receiver, SymbolId, u32)> {
    raw.uses
        .iter()
        .map(|used| {
            assert_eq!(used.access, Access::Read, "{used:?}");
            (used.name.as_str(), &used.receiver, used.site_of, used.line)
        })
        .collect()
}

#[test]
fn a_method_named_by_a_symbol_is_a_use_of_it() {
    let raw = extract(
        b"class Feed
  def boot
    items.map(&method(:render))
    send(:refresh, value)
    self.public_send(:refresh)
    Cache.__send__(:flush)
    Cache::Store.method(:open)
    other.send(:render)
    respond_to?(:render)
    Feed.instance_method(:render)
    public_method(:render)
    singleton_method(:build)
  end
  define_method(:reload) { send(:refresh) }
  singleton_method(:make)
end
send(:setup)
def run
  send(:setup)
end
def obj.tick
  send(:setup)
end
def self.start
  Cache.singleton_method(:warm)
  send(:setup)
end
",
    );

    let (feed, boot, run, tick, start) = (
        id_of(&raw.symbols, "Feed"),
        id_of(&raw.symbols, "Feed#boot"),
        id_of(&raw.symbols, "run"),
        id_of(&raw.symbols, "tick"),
        id_of(&raw.symbols, "start"),
    );
    assert_eq!(
        uses(&raw),
        [
            ("render", &Receiver::Path("Feed".to_owned()), boot, 3),
            ("refresh", &Receiver::Path("Feed".to_owned()), boot, 4),
            ("refresh", &Receiver::Path("Feed".to_owned()), boot, 5),
            ("flush", &Receiver::Path("Cache.".to_owned()), boot, 6),
            ("open", &Receiver::Path("Cache::Store.".to_owned()), boot, 7),
            ("render", &Receiver::Path("Feed".to_owned()), boot, 11),
            ("refresh", &Receiver::Path("Feed.".to_owned()), feed, 14),
            ("make", &Receiver::Path("Feed.".to_owned()), feed, 15),
            ("setup", &Receiver::Bare, FILE_SCOPE, 17),
            ("setup", &Receiver::Bare, run, 19),
            ("setup", &Receiver::Bare, tick, 22),
            ("warm", &Receiver::Path("Cache.".to_owned()), start, 25),
            ("setup", &Receiver::Bare, start, 26),
        ]
    );
}

#[test]
fn a_type_named_as_one_is_no_use_of_it() {
    let raw = extract(
        b"require 'shapes/shape'
class Circle < Shape
  include Drawable
  def grow(size)
    raise ArgumentError unless size.is_a?(Size)
    Circle.new(Size::DEFAULT)
  rescue Shapes::Error
    nil
  end
end
",
    );

    let declared = names(&raw.symbols);
    for name in ["Circle", "Circle#grow"] {
        assert!(declared.contains(&name), "{name} in {declared:?}");
    }
    assert_eq!(uses(&raw), []);
    assert_eq!(Ruby::new().conventions().type_uses, TypeUses::Nowhere);
}

#[test]
fn a_read_of_a_constant_is_no_use_of_it() {
    let raw = extract(
        b"class Feed
  MAX = 10
  def limit
    MAX + Feed::MAX
  end
end
",
    );

    assert!(
        raw.symbols
            .iter()
            .any(|symbol| symbol.kind == SymbolKind::Constant && symbol.name == "MAX"),
        "{:?}",
        names(&raw.symbols)
    );
    assert_eq!(uses(&raw), []);
    assert_eq!(
        Ruby::new().conventions().unrecorded_data,
        [SymbolKind::Constant]
    );
}

#[test]
fn a_signature_leaves_out_its_comments() {
    let raw = extract(
        b"def walk(a, # the first\n  b)\n  a\nend\n\ndef pair(a,\n=begin\nthe second\n=end\n  b)\n  b\nend\n",
    );

    assert_eq!(
        symbol(&raw.symbols, "walk").signature.as_deref(),
        Some("def walk(a,\n  b)")
    );
    assert_eq!(
        symbol(&raw.symbols, "pair").signature.as_deref(),
        Some("def pair(a,\n\n  b)")
    );
}
