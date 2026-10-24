//! How sure each tier of the call cascade is of the symbol it binds, from 95 down to 15.
//!
//! A call or a use is resolved by the first tier that binds it, and the answer carries that tier's confidence: a
//! [`Limits`](crate::Limits) floor of [`INCLUDE`] keeps the calls bound through an include and every tier above it.

/// A receiver that names an import binds an export of the imported directory, or what the imported module's file shows
/// the call.
///
/// A call of a name imported from a module's file binds what that file shows it under the name. The file shows an
/// export to any call, a definition it does not export to a call from the file or a file it includes, directly or
/// through others, and a test definition only to test code that shares an include root with the file. A dot import
/// binds where one dot import answers the name. A receiver that names the module an included file declares binds that
/// file's definition of the name.
pub const IMPORT: u8 = 95;
/// A callee or a namespace's value written qualified binds the symbol of that qualified name.
pub const QUALIFIED: u8 = 90;
/// A callee written with more scopes or fewer than a qualified name keeps binds by its trailing owner and name.
///
/// A qualified callee written inside a namespace binds first, ahead of the qualified name it spells, the definition
/// that its innermost enclosing scope names and the caller's file sees: `time_internal::f` called inside `absl` binds
/// `absl::time_internal::f`.
///
/// `ns::inner::Widget::draw` binds `Widget::draw`, and `Widget::Kind` binds a member of `Widget` of a longer qualified
/// name, `ns::Widget::Kind`, one that the caller's file sees first. A namespace's value written inside an enclosing
/// namespace, `detail::scale` inside `cfg`, binds `cfg::detail::scale` here.
pub const QUALIFIED_TAIL: u8 = 88;
/// A receiver that names an owner binds that owner's member.
pub const RECEIVER: u8 = 85;
/// A local, or a definition in the caller's own file, binds; so does a member of the caller's owner on a written
/// receiver that names no owner.
///
/// The caller's owner binds only once the scope tiers bind nothing, and where a language lists
/// [`kartta_lang::Conventions::own_type_receivers`], only on one of those receivers.
///
/// A call on a bare name that the language writes only for a class, of which the graph holds no type, binds neither
/// here nor at any tier below the include tiers.
pub const SAME_FILE: u8 = 80;
/// A definition in the caller's package directory binds, where a language's packages are directories.
pub const PACKAGE: u8 = 75;
/// The one file of those the caller's file includes that defines the name binds.
///
/// Where the language's includes are transitive and none of those defines the name, the one file of those the
/// includes reach in turn binds a name few definitions share.
pub const INCLUDE: u8 = 70;
/// A re-export chain from an included file binds the definition it renames.
pub const RE_EXPORT: u8 = 65;
/// The one callable of the name binds from anywhere, and where the language's includes are transitive, from a file
/// that reaches it or a declaration of it, or that a file reaching one includes directly.
///
/// A call of a method the language's runtime gives its values binds here only where its receiver spells that
/// callable's owner in any case: `session.seek()` binds the one `seek`, `Session.seek`. A method call of a method the
/// language's standard library declares, on a value the graph cannot type, binds here only where its receiver spells
/// the callable's owner, and otherwise at [`FIRST_PARTY`]: `found.start()` binds `Watcher::start` as a guess. A call
/// from outside test code binds no test definition here, and a test definition still counts among the definitions of
/// its name, except against the one type outside test code, which such a call binds.
pub const UNIQUE: u8 = 60;
/// The one callable of the name in the caller's directory binds.
///
/// Here and at the two steps after it, a call from outside test code binds no test definition, and a test definition
/// still counts among the definitions of its name nearest the caller.
pub const SAME_DIR: u8 = 50;
/// The one callable of the name in the caller's parent directory binds.
pub const PARENT_DIR: u8 = 30;
/// The one callable of the name outside vendored code binds.
pub const FIRST_PARTY: u8 = 15;
