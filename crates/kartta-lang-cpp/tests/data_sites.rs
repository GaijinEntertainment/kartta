//! The data sites a C++ body or a file-scope initializer yields: each field access, each read or write of a name no
//! declaration of the body binds, how it touches its target and what its name is written on.

use kartta_lang::{Access, FILE_SCOPE, Language, RawExtraction, Receiver, TypeUses};
use kartta_lang_cpp::Cpp;

fn extract(source: &str) -> RawExtraction {
    Cpp::new().extract(source.as_bytes())
}

fn sites(raw: &RawExtraction) -> Vec<(&str, Receiver, Access)> {
    raw.uses
        .iter()
        .map(|site| (site.name.as_str(), site.receiver.clone(), site.access))
        .collect()
}

fn named(raw: &RawExtraction, name: &str) -> Vec<(Receiver, Access, u32)> {
    raw.uses
        .iter()
        .filter(|site| site.name == name)
        .map(|site| (site.receiver.clone(), site.access, site.line))
        .collect()
}

fn path(owner: &str) -> Receiver {
    Receiver::Path(owner.to_owned())
}

#[test]
fn a_member_access_names_the_type_its_operand_was_declared_with() {
    let raw = extract(
        "template <typename T>
void use(const Config &config, Config *pointer, T &generic, ns::Box<Item> boxed) {
    Config local;
    auto deduced = make();
    read(config.limit, pointer->limit, pointer.limit, local.limit, deduced.limit);
    read(generic.limit, boxed.limit, config.inner.limit, global.limit);
}
",
    );

    let limits: Vec<Receiver> = named(&raw, "limit")
        .into_iter()
        .map(|(receiver, _, _)| receiver)
        .collect();
    assert_eq!(
        limits,
        [
            path("Config"),
            path("Config"),
            path(""),
            path("Config"),
            path(""),
            path(""),
            path("Box"),
            path(""),
            path(""),
        ],
        "`.` takes a value or a reference, `->` a pointer; a deduced, generic or chained operand names no type"
    );
    assert_eq!(named(&raw, "inner"), [(path("Config"), Access::Read, 6)]);
}

#[test]
fn a_qualified_name_read_without_a_call_uses_its_terminal_on_the_scope_s_last_segment() {
    let raw = extract(
        "void use() {
    register_handler(&Widget::draw, Config::kLimit, ns::Box<int>::empty);
    Config::kLimit = ::global_limit + outer::inner::value;
    Widget::draw();
    ns::Type local;
}
",
    );

    assert_eq!(
        sites(&raw),
        [
            ("draw", path("Widget"), Access::Read),
            ("kLimit", path("Config"), Access::Read),
            ("empty", path("Box"), Access::Read),
            ("kLimit", path("Config"), Access::Write),
            ("global_limit", Receiver::Bare, Access::Read),
            ("value", path("inner"), Access::Read),
        ],
        "a callee and a type are no use, and a name written from the root is bare"
    );
}

#[test]
fn a_member_of_this_is_on_the_own_receiver_and_a_called_member_is_no_site() {
    let raw = extract(
        "void Widget::resize(int width) {
    this->width = width;
    this->layout();
    other.paint();
}
",
    );

    assert_eq!(
        sites(&raw),
        [
            ("width", Receiver::Own, Access::Write),
            ("other", Receiver::Bare, Access::Read)
        ]
    );
}

#[test]
fn a_name_no_declaration_of_the_body_binds_is_a_bare_site() {
    let raw = extract(
        "int counter;
void Widget::tick(Item &item, int (*hook)(int), Args &&...rest) {
    int sum = counter + limit;
    for (auto &[key, value] : table) { sum += key + value; }
    for (Item &each : items) { sum += each.size; }
    auto scaled = [factor = scale](int x) { return x * factor * sum; };
    try { run(item, hook, rest); } catch (const Error &error) { report(error, ns::fallback); }
    goto done;
done:
    return;
}
",
    );

    let mut names: Vec<&str> = raw
        .uses
        .iter()
        .filter(|site| site.receiver == Receiver::Bare)
        .map(|site| site.name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["counter", "items", "limit", "scale", "table"],
        "locals, parameters, bindings, captures and handlers declare their names, and callees, qualified names and \
         labels are no value of the file's"
    );
}

#[test]
fn a_name_is_bare_until_the_body_declares_it_and_after_its_block_ends() {
    let raw = extract(
        "void f() {
    total = 1;
    {
        int total = 2;
        total += 1;
    }
    total = 3;
}
",
    );

    assert_eq!(
        named(&raw, "total"),
        [
            (Receiver::Bare, Access::Write, 2),
            (Receiver::Bare, Access::Write, 7)
        ]
    );
}

#[test]
fn a_template_parameter_names_no_value_and_no_type() {
    let raw = extract(
        "template <typename T, int Size>
struct Buffer {
    int capacity() const { return Size * stride; }
    void copy(const T &from) { width = from.width; }
};
",
    );

    assert_eq!(
        sites(&raw),
        [
            ("stride", Receiver::Bare, Access::Read),
            ("width", Receiver::Bare, Access::Write),
            ("width", path(""), Access::Read)
        ]
    );
}

#[test]
fn only_the_outermost_target_of_an_assignment_is_written() {
    let raw = extract(
        "void f(State &state) {
    state.inner.count = 1;
    state.total += 2;
    ++state.ticks;
    (state.flag) = true;
    other = state.total;
}
",
    );

    assert_eq!(
        sites(&raw),
        [
            ("count", path(""), Access::Write),
            ("inner", path("State"), Access::Read),
            ("total", path("State"), Access::Update),
            ("ticks", path("State"), Access::Update),
            ("flag", path("State"), Access::Write),
            ("other", Receiver::Bare, Access::Write),
            ("total", path("State"), Access::Read),
        ]
    );
}

#[test]
fn an_initializer_list_and_a_designated_initializer_write_their_fields() {
    let raw = extract(
        "Widget::Widget(int width) : width(width), Base(1), height{2} {
    Rect rect = {.left = 0, .top = origin};
    Rect nested[] = {{.left = 1}};
    use(Rect{.left = 3});
}
",
    );

    assert_eq!(
        sites(&raw),
        [
            ("width", Receiver::Own, Access::Write),
            ("Base", Receiver::Own, Access::Write),
            ("height", Receiver::Own, Access::Write),
            ("left", path("Rect"), Access::Write),
            ("top", path("Rect"), Access::Write),
            ("origin", Receiver::Bare, Access::Read),
            ("left", path(""), Access::Write),
            ("left", path("Rect"), Access::Write),
        ]
    );
}

#[test]
fn a_site_belongs_to_the_definition_around_it_and_a_file_scope_initializer_to_the_file() {
    let raw = extract(
        "int table[] = {first, &Widget::second};
Handler handler = on_event;
struct Slot { Handler handler = on_member; };
auto lambda = [](int local) { return local; };
PX_FORCE_INLINE void Widget::draw(int width = fallback) {
    auto paint = [this]() { return colour; };
}
",
    );

    let draw = raw
        .symbols
        .iter()
        .position(|symbol| symbol.name == "draw")
        .expect("the definition is a symbol");
    let held: Vec<(&str, Option<usize>)> = raw
        .uses
        .iter()
        .map(|site| {
            let scope = (site.site_of != FILE_SCOPE).then_some(site.site_of.0 as usize);
            (site.name.as_str(), scope)
        })
        .collect();
    assert_eq!(
        held,
        [
            ("first", None),
            ("second", None),
            ("on_event", None),
            ("on_member", None),
            ("colour", Some(draw))
        ],
        "an initializer outside every body names what the file's scope reaches, a lambda's body belongs to the \
         definition that holds it, and a lambda outside one, a default argument and an annotation macro in a \
         definition's head yield nothing"
    );
}

#[test]
fn a_type_named_as_one_is_no_use_of_it() {
    let raw = extract(
        "struct Point { int x; };
using Alias = Point;
class Line : public Shape { Point from; };
Point draw(const Line &line, Alias *alias) {
    Point start = line.from;
    auto copy = static_cast<Point>(start);
    std::vector<Line> lines;
    return Point{copy.x};
}
",
    );

    assert_eq!(
        sites(&raw),
        [
            ("from", path("Line"), Access::Read),
            ("x", path(""), Access::Read)
        ]
    );
    assert_eq!(Cpp::new().conventions().type_uses, TypeUses::Nowhere);
}
