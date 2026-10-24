//! How a C++ spelling picks one class among namesakes: classes of one bare name in several scopes, and a template with
//! its specializations, which share one qualified name.
//!
//! - Of a template and its specializations, a spelling's template arguments pick, scope by scope from its class
//!   outward: the explicit specialization whose head takes the same arguments one for one, whatever spaces they hold,
//!   else a partial specialization whose head takes them as written, else the primary template, which a spelling
//!   without arguments names too. Beside a partial specialization, whose pattern may take other arguments, a
//!   declaration's arguments that name only the primary pick none. A constrained partial specialization whose
//!   arguments are its own parameters, `requires is_void_v<T> class Expected<T, E>`, is picked by no spelling.
//! - The choosers run in this order: a declaration's template arguments; the class the use's function defines, or whose
//!   body holds the use; the own class, the member's class; the arguments that the scope of a member defined outside
//!   its class writes, `void Box<bool>::flip()`, which name its own class even beside a partial specialization.

use std::borrow::Cow;

use crate::resolve::{Cascade, derives};
use crate::stored::{StoredSymbol, SymbolAt};
use crate::strings::Str;

/// What a receiver's spelling picks among the members of one owner's name that a tier of the caller's reach holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Twin {
    /// The member of the class the spelling names, or of the one class the members belong to.
    Member(SymbolAt),
    /// The first member of classes of one qualified name, which one spelling names alike: a template and its
    /// specializations, a class under each branch of a conditional, or copies of one class.
    Namesake(SymbolAt),
    /// The tier's first member, where classes of other qualified names hold the members and the spelling names none.
    Unnamed(SymbolAt),
}

/// A specialization's head as the pick reads it: `template <typename T> requires C<T> struct Box<T *> : Base`.
#[derive(Debug, Clone, Copy)]
struct Head<'h> {
    /// The template parameters it declares, empty for an explicit specialization's `template <>`.
    parameters: &'h str,
    /// The template arguments it gives its class.
    taken: &'h str,
    /// Whether it states a constraint.
    constrained: bool,
}

impl<'h> Head<'h> {
    /// The head `written` of the class `name`; `None` where it gives `name` no template arguments.
    fn read(written: &'h str, name: &str) -> Option<Self> {
        let mut rest = written.trim_start();
        let mut parameters = "";
        if let Some(after) = rest.strip_prefix("template") {
            let after = after.trim_start();
            parameters = closed_list(after)?;
            rest = &after[parameters.len() + 2..];
        }
        let mut from = 0;
        while let Some(found) = rest[from..].find(name) {
            let at = from + found;
            let before = rest[..at].bytes().next_back();
            let after = rest[at + name.len()..].trim_start();
            if !before.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                && let Some(taken) = closed_list(after)
            {
                let constrained = written.contains("requires");
                return Some(Self {
                    parameters,
                    taken,
                    constrained,
                });
            }
            from = at + name.len();
        }
        None
    }

    fn is_partial(self) -> bool {
        !self.parameters.trim().is_empty()
    }

    // constraint: arguments that are the parameters, `template <typename Base> class Ptr<Base>`, still specialize a
    // primary of more parameters, `Ptr<Base, Versions...>`; only a constraint makes them its primary's own
    /// Whether a constrained partial specialization's arguments are its own parameters, each once in order, which only
    /// its constraint tells from its primary's.
    fn takes_only_its_parameters(self) -> bool {
        if !self.constrained {
            return false;
        }
        let mut names = split_list(self.parameters)
            .map(|parameter| {
                let declared = parameter.split('=').next().unwrap_or(parameter).trim_end();
                let start = declared
                    .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .map_or(0, |at| at + 1);
                &declared[start..]
            })
            .peekable();
        names.peek().is_some()
            && names
                .eq(split_list(self.taken)
                    .map(|argument| argument.trim_end_matches("...").trim_end()))
    }
}

/// The items of a list at its outermost commas.
fn split_list(list: &str) -> impl Iterator<Item = &str> {
    let mut nesting = Nesting::default();
    list.split(move |c: char| {
        if c.is_ascii() {
            nesting.step(c as u8);
        }
        c == ',' && nesting.is_outside()
    })
    .map(str::trim)
    .filter(|item| !item.is_empty())
}

/// What spells the template arguments a class is picked by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelled {
    /// A receiver's declared type, which may name a partial specialization's pattern by other arguments.
    Receiver,
    /// The scope a member defined outside its class writes, which names its class's own arguments.
    Scope,
}

/// How a class takes the template arguments a receiver spells, the closest match greatest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    /// The class, or a class around it, takes other arguments than the receiver spells, or some where it spells none.
    Other,
    /// A primary template where the receiver spells arguments, and the same arguments everywhere else.
    Primary,
    /// The arguments the receiver spells at every scope, and none where it spells none.
    Taken,
}

impl Twin {
    /// The member the pick carries, whatever it is.
    pub(crate) const fn member(self) -> SymbolAt {
        match self {
            Self::Member(at) | Self::Namesake(at) | Self::Unnamed(at) => at,
        }
    }
}

impl<'g> Cascade<'g> {
    /// The owner a receiver names, `Widget` for `b::Widget`, since a member keeps its class's bare name as its owner;
    /// `None` where no symbol has that name.
    pub(crate) fn bare_owner(&self, receiver: Str) -> Option<Str> {
        let Some(separator) = self.conventions.scope_separator else {
            return Some(receiver);
        };
        let written = self.graph.strings.resolve(receiver);
        let class = unargued(written);
        match class.rsplit_once(separator) {
            Some((_, bare)) => self.graph.strings.get(bare),
            None if class.len() < written.len() => self.graph.strings.get(class),
            None => Some(receiver),
        }
    }

    // constraint: C++ looks a class's name up from the scope around the use outward, so of the classes that hold the
    // tier's members, the receiver's spelling names those the innermost scope around the use holds under it; where it
    // names none, classes of one qualified name are still one name's
    pub(crate) fn twin(&self, site: SymbolAt, written: &str, tier: &[SymbolAt]) -> Option<Twin> {
        let &first = tier.first()?;
        if tier.len() == 1 {
            return Some(Twin::Member(first));
        }
        let classes: Vec<(SymbolAt, SymbolAt)> = tier
            .iter()
            .filter_map(|&at| Some((at, self.class_holding(at)?)))
            .collect();
        let one = classes.first().map(|&(_, class)| class);
        if classes.iter().all(|&(_, class)| Some(class) == one) {
            return Some(Twin::Member(first));
        }
        let class = unargued(written);
        let named = self.named_by(site, class, &classes);
        let separator = self.conventions.scope_separator.unwrap_or("::");
        let elsewhere = named.is_empty() && class.contains(separator);
        let held = if named.is_empty() { classes } else { named };
        let arguments = (!elsewhere).then_some(written);
        Some(self.namesake((site, arguments), first, &in_site_file(site, held)))
    }

    /// The class a member belongs to, or a class itself; `None` for a type that holds no members, an alias.
    fn class_holding(&self, at: SymbolAt) -> Option<SymbolAt> {
        let kind = self.graph.sym(at).kind;
        if kind.is_type() {
            return derives(kind).then_some(at);
        }
        self.class_of(at)
    }

    fn named_by(
        &self,
        site: SymbolAt,
        written: &str,
        classes: &[(SymbolAt, SymbolAt)],
    ) -> Vec<(SymbolAt, SymbolAt)> {
        let separator = self.conventions.scope_separator.unwrap_or("::");
        let scope = self.scope_around(site);
        let depths: Vec<Option<usize>> = classes
            .iter()
            .map(|&(_, class)| holder_depth(self.qualified_name(class), written, scope, separator))
            .collect();
        let Some(nearest) = depths.iter().flatten().max() else {
            return Vec::new();
        };
        classes
            .iter()
            .zip(&depths)
            .filter(|&(_, depth)| depth.as_ref() == Some(nearest))
            .map(|(&held, _)| held)
            .collect()
    }

    /// The scope a use looks a name up from: a member's class, whose qualified name keeps the namespaces the member's
    /// own leaves out, else the scope the use's function is qualified by; empty at file scope.
    pub(crate) fn scope_around(&self, site: SymbolAt) -> &'g str {
        let separator = self.conventions.scope_separator.unwrap_or("::");
        if self.site_symbol(site).is_none() {
            return "";
        }
        let body = self.holder(site).unwrap_or(site);
        self.graph
            .sym(body)
            .owner
            .and_then(|_| self.class_of(body))
            .map(|class| self.qualified_name(class))
            .or_else(|| self.graph.enclosing_scope(body, separator))
            .unwrap_or("")
    }

    // constraint: a class the use's function defines, or one whose body holds the use, is the one its names mean
    // before a namesake elsewhere, unless the receiver's template arguments name another; a template and its
    // specializations share one qualified name, so the arguments the receiver writes pick between them, where its
    // scopes name the classes
    fn namesake(
        &self,
        (site, written): (SymbolAt, Option<&str>),
        first: SymbolAt,
        held: &[(SymbolAt, SymbolAt)],
    ) -> Twin {
        if let Some(member) = one_class(held.iter().copied()) {
            return Twin::Member(member);
        }
        let argued = written.is_some_and(|written| written.contains('<'));
        let specialization =
            || written.and_then(|written| self.specialization((written, Spelled::Receiver), held));
        if argued && let Some(twin) = specialization() {
            return twin;
        }
        if let Some(symbol) = self.site_symbol(site) {
            let here = || {
                held.iter()
                    .copied()
                    .filter(|&(_, class)| class.file == site.file)
            };
            let defined = here().filter(|&(_, class)| {
                (symbol.line..=symbol.end_line).contains(&self.graph.sym(class).line)
            });
            let around = here().filter(|&(_, class)| {
                let class = self.graph.sym(class);
                (class.line..=class.end_line).contains(&symbol.line)
            });
            if let Some(member) = one_class(defined).or_else(|| one_class(around)) {
                return Twin::Member(member);
            }
        }
        let body = self.holder(site).unwrap_or(site);
        let own = self
            .site_symbol(body)
            .and_then(|symbol| symbol.owner)
            .and_then(|_| self.class_of(body));
        if let Some(own) = own
            && let Some(&(member, _)) = held.iter().find(|&&(_, class)| class == own)
        {
            return Twin::Member(member);
        }
        let scope = written
            .filter(|_| !argued)
            .and_then(|written| self.own_scope(body, written));
        if let Some(scope) = scope
            && let Some(twin) = self.specialization((scope, Spelled::Scope), held)
        {
            return twin;
        }
        if !argued
            && scope.is_none()
            && let Some(twin) = specialization()
        {
            return twin;
        }
        let Some(&(member, class)) = held.first() else {
            return Twin::Unnamed(first);
        };
        let name = self.graph.sym(class).qualified_name;
        if held
            .iter()
            .all(|&(_, other)| self.graph.sym(other).qualified_name == name)
        {
            Twin::Namesake(member)
        } else {
            Twin::Unnamed(first)
        }
    }

    // constraint: a specialization keeps its head as its signature, so a class without one is a primary template, or a
    // class no template makes; a receiver's arguments, segment by segment from its class outward, name the
    // specialization that takes the same, else the primary at each scope that takes them; a partial specialization's
    // arguments are a pattern that other arguments may match, so beside one a receiver's arguments name the primary
    // only where they are spelled as a member's scope, which names its own class's
    /// The member of the class of `held`, classes of one qualified name, that the template arguments `written` spells
    /// name; `None` where they name none, or where the classes are of several qualified names.
    fn specialization(
        &self,
        (written, spelled): (&str, Spelled),
        held: &[(SymbolAt, SymbolAt)],
    ) -> Option<Twin> {
        let &(_, class) = held.first()?;
        let name = self.graph.sym(class).qualified_name;
        if held
            .iter()
            .any(|&(_, other)| self.graph.sym(other).qualified_name != name)
        {
            return None;
        }
        let arguments = arguments(written);
        let mut partial = false;
        let mut best: Option<(Rank, SymbolAt, SymbolAt, bool)> = None;
        for &(member, class) in held {
            let (rank, of_partial) = self.rank(class, &arguments);
            partial |= of_partial;
            best = match best {
                Some((top, first, of, one)) if rank == top => {
                    Some((top, first, of, one && of == class))
                }
                Some(kept) if rank < kept.0 => Some(kept),
                _ => Some((rank, member, class, true)),
            };
        }
        let (rank, member, _, one) = best.filter(|&(rank, ..)| rank > Rank::Other)?;
        if spelled == Spelled::Receiver && rank == Rank::Primary && partial {
            return None;
        }
        Some(if one {
            Twin::Member(member)
        } else {
            Twin::Namesake(member)
        })
    }

    /// How the arguments a receiver spells, its class's first, match the ones `class` and the classes around it take,
    /// and whether `class` is a partial specialization, one whose head declares template parameters.
    fn rank(&self, class: SymbolAt, arguments: &[Option<&str>]) -> (Rank, bool) {
        let (mut rank, mut partial_class) = (Rank::Taken, None);
        let mut level = Some(class);
        for &written in arguments {
            let Some(at) = level else {
                break;
            };
            let symbol = self.graph.sym(at);
            let strings = &self.graph.strings;
            let head = symbol
                .signature
                .and_then(|head| Head::read(strings.resolve(head), strings.resolve(symbol.name)));
            let own = *partial_class.get_or_insert_with(|| head.is_some_and(Head::is_partial));
            match (written, head) {
                (Some(_), None) => rank = Rank::Primary,
                (None, None) => {}
                (Some(written), Some(head))
                    if same_arguments(written, head.taken) && !head.takes_only_its_parameters() => {
                }
                _ => return (Rank::Other, own),
            }
            level = self.class_of(at);
        }
        (rank, partial_class.unwrap_or(false))
    }

    /// The class a member defined outside its class names as its scope, with the template arguments its signature
    /// writes, `Box<bool>` of `void Box<bool>::flip()`, where `written` is that class's bare name.
    fn own_scope(&self, body: SymbolAt, written: &str) -> Option<&'g str> {
        let symbol = self.site_symbol(body)?;
        let strings = &self.graph.strings;
        if strings.resolve(symbol.owner?) != written {
            return None;
        }
        let signature = strings.resolve(symbol.signature?);
        let name = strings.resolve(symbol.name);
        let mut from = 0;
        while let Some(found) = signature[from..].find(written) {
            let at = from + found;
            let after = &signature[at + written.len()..];
            let before = signature[..at].bytes().next_back();
            if !before.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                && let Some(list) = closed_list(after)
                && after[list.len() + 2..]
                    .trim_start()
                    .strip_prefix("::")
                    .is_some_and(|member| member.trim_start().starts_with(name))
            {
                return Some(&signature[at..at + written.len() + list.len() + 2]);
            }
            from = at + written.len();
        }
        None
    }

    fn site_symbol(&self, site: SymbolAt) -> Option<&'g StoredSymbol> {
        self.graph.file(site.file).symbols.get(site.index as usize)
    }

    fn qualified_name(&self, class: SymbolAt) -> &'g str {
        self.graph
            .strings
            .resolve(self.graph.sym(class).qualified_name)
    }
}

/// The members of `owned` that `held` keeps, borrowed where it keeps one, so a tier of one member allocates nothing.
pub(crate) fn kept(owned: &[SymbolAt], held: impl Fn(&SymbolAt) -> bool) -> Cow<'_, [SymbolAt]> {
    let mut found = owned.iter().filter(|&at| held(at));
    match (found.next(), found.next()) {
        (None, _) => Cow::Borrowed(&[]),
        (Some(one), None) => Cow::Borrowed(std::slice::from_ref(one)),
        (Some(&one), Some(&two)) => [one, two].into_iter().chain(found.copied()).collect(),
    }
}

/// The member of the first pair where every pair holds the same class.
fn one_class(mut held: impl Iterator<Item = (SymbolAt, SymbolAt)>) -> Option<SymbolAt> {
    let (member, class) = held.next()?;
    held.all(|(_, other)| other == class).then_some(member)
}

// constraint: a member the use's own file declares is the one the use sees first, as a definition is before the
// declaration its file includes
fn in_site_file(site: SymbolAt, held: Vec<(SymbolAt, SymbolAt)>) -> Vec<(SymbolAt, SymbolAt)> {
    if !held.iter().any(|&(member, _)| member.file == site.file) {
        return held;
    }
    held.into_iter()
        .filter(|&(member, _)| member.file == site.file)
        .collect()
}

/// The byte ranges of a spelling's scopes, split at each `::` outside its template arguments, the empty one before a
/// `::` that roots it included.
fn segments(written: &str) -> Vec<(usize, usize)> {
    let bytes = written.as_bytes();
    let mut spans = Vec::new();
    let (mut nesting, mut start, mut at) = (Nesting::default(), 0, 0);
    while at < bytes.len() {
        nesting.step(bytes[at]);
        if bytes[at] == b':' && nesting.is_outside() && bytes.get(at + 1) == Some(&b':') {
            spans.push((start, at));
            start = at + 2;
            at += 1;
        }
        at += 1;
    }
    spans.push((start, bytes.len()));
    spans
}

/// The template arguments one scope of a spelling takes, `bool` of `Box<bool>`.
fn segment_arguments(segment: &str) -> Option<&str> {
    let open = segment.find('<')?;
    segment[open + 1..].strip_suffix('>')
}

/// The part of a receiver's spelling that names its class by scopes alone, as a class's qualified name does: `ns::Box`
/// of `ns::Box<bool>`, and past the last scope that takes arguments, `Inner` of `Outer<int>::Inner`.
pub(crate) fn unargued(written: &str) -> &str {
    if !written.contains('<') {
        return written;
    }
    let spans = segments(written);
    let Some((&(start, end), outer)) = spans.split_last() else {
        return written;
    };
    let end = written[start..end]
        .find('<')
        .map_or(end, |open| start + open);
    let begin = outer
        .iter()
        .rfind(|&&(start, end)| segment_arguments(&written[start..end]).is_some())
        .map_or(0, |&(_, end)| end + 2);
    &written[begin.min(end)..end]
}

/// The template arguments a receiver's spelling writes at each of its scopes, its class's first and the outermost
/// last, each `None` where the scope takes none.
fn arguments(written: &str) -> Vec<Option<&str>> {
    segments(written)
        .iter()
        .rev()
        .filter(|&&(start, end)| start < end)
        .map(|&(start, end)| segment_arguments(&written[start..end]))
        .collect()
}

/// The inside of the template argument list `written` opens with, up to the `>` that closes it.
fn closed_list(written: &str) -> Option<&str> {
    if !written.starts_with('<') {
        return None;
    }
    let mut nesting = Nesting::default();
    written.bytes().enumerate().find_map(|(at, byte)| {
        nesting.step(byte);
        (byte == b'>' && nesting.is_outside()).then(|| &written[1..at])
    })
}

/// How deep a spelling's template argument lists and parentheses nest at a byte, so a `>` inside parentheses,
/// `Less<(1 > 2)>`, closes no list.
#[derive(Debug, Default, Clone, Copy)]
struct Nesting {
    angles: usize,
    parentheses: usize,
}

impl Nesting {
    const fn step(&mut self, byte: u8) {
        match byte {
            b'(' => self.parentheses += 1,
            b')' => self.parentheses = self.parentheses.saturating_sub(1),
            b'<' if self.parentheses == 0 => self.angles += 1,
            b'>' if self.parentheses == 0 => self.angles = self.angles.saturating_sub(1),
            _ => {}
        }
    }

    const fn is_outside(self) -> bool {
        self.angles == 0 && self.parentheses == 0
    }
}

// constraint: the graph holds no primary template's head, so a spelling that leaves trailing arguments to their
// defaults, `BsonType<const char *>`, cannot be told to name the specialization `BsonType<const char *, void>`
/// Whether the arguments a spelling writes are the ones a specialization's head takes, one for one, whatever spaces
/// each is written with.
fn same_arguments(written: &str, taken: &str) -> bool {
    let mut taken = split_list(taken);
    split_list(written).all(|one| taken.next().is_some_and(|other| same_spelling(one, other)))
        && taken.next().is_none()
}

fn same_spelling(left: &str, right: &str) -> bool {
    left.bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .eq(right.bytes().filter(|byte| !byte.is_ascii_whitespace()))
}

/// How far into the scope around a use the scope reaches that holds a class of `qualified` name under the spelling
/// `written`, by the length of its name, the global scope at 0; `None` where no scope around the use holds it so.
pub(crate) fn holder_depth(
    qualified: &str,
    written: &str,
    scope: &str,
    separator: &str,
) -> Option<usize> {
    if let Some(rooted) = written.strip_prefix(separator) {
        return (qualified == rooted).then_some(0);
    }
    let holder = if qualified == written {
        ""
    } else {
        qualified.strip_suffix(written)?.strip_suffix(separator)?
    };
    let encloses = holder.is_empty()
        || scope
            .strip_prefix(holder)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(separator));
    encloses.then_some(holder.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spelling_names_its_class_by_the_scopes_past_the_last_that_takes_arguments() {
        let classes: Vec<&str> = [
            "b::Widget",
            "::b::Widget",
            "ns::Box<bool>",
            "ns::Box<a::B<c>, d>",
            "Outer<int>::Inner",
            "A<X>::b::C<Y>",
        ]
        .into_iter()
        .map(unargued)
        .collect();

        assert_eq!(
            classes,
            [
                "b::Widget",
                "::b::Widget",
                "ns::Box",
                "ns::Box",
                "Inner",
                "b::C"
            ]
        );
    }

    #[test]
    fn a_spelling_s_arguments_run_from_its_class_outward() {
        assert_eq!(arguments("ns::Box<a::B<c>, d>"), [Some("a::B<c>, d"), None]);
        assert_eq!(arguments("::Outer<int>::Inner"), [None, Some("int")]);
        assert_eq!(arguments("Widget"), [None]);
        assert_eq!(arguments("Less<(1 > 2)>::Inner"), [None, Some("(1 > 2)")]);
    }

    #[test]
    fn a_specialization_s_head_gives_the_arguments_after_its_name() {
        let taken = |head| Head::read(head, "Box").map(|head| head.taken);

        assert_eq!(
            taken("template <> struct Box<bool> : Base<int>"),
            Some("bool")
        );
        assert_eq!(
            taken("template <typename T, typename = Box<T>> struct Box<T *>"),
            Some("T *")
        );
        assert_eq!(
            taken("struct EXPORT(Box) Box< Map<int, char> >"),
            Some(" Map<int, char> ")
        );
        assert_eq!(taken("struct ToolBox<int>"), None);
        assert_eq!(taken("template <> struct Box<(1 > 2)>"), Some("(1 > 2)"));
    }

    #[test]
    fn only_a_constraint_makes_a_partial_specialization_s_parameters_its_primary_s_own() {
        let own = |head, name| Head::read(head, name).is_some_and(Head::takes_only_its_parameters);

        assert!(own(
            "template <typename T, typename E> requires eastl::is_void_v<T> class Expected<T, E>",
            "Expected"
        ));
        assert!(!own("template <typename Base> class Ptr<Base>", "Ptr"));
        assert!(!own(
            "template <typename T> struct Box<T *> requires true",
            "Box"
        ));
    }

    #[test]
    fn a_spelling_takes_a_head_s_arguments_one_for_one_whatever_its_spaces() {
        assert!(same_arguments(" Map<int, char> ", "Map<int,char>"));
        assert!(same_arguments("", ""));
        assert!(!same_arguments("const char *", "const char *, void"));
        assert!(!same_arguments("bool, double", "bool"));
        assert!(!same_arguments("int, char", "int, void"));
    }
}
