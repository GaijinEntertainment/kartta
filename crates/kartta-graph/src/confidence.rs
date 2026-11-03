//! How sure each tier of the call cascade is of the symbol it binds, from 95 down to 15.
//!
//! A call or a use is resolved by the first tier that binds it, and the answer carries that tier's confidence: a
//! [`Limits`](crate::Limits) floor of [`INCLUDE`] keeps the calls bound through an include and every tier above it.
//!
//! A call the cascade binds to a definition a macro invocation writes, or leaves ambiguous where such a definition
//! shares its name, runs the cascade again over the written definitions alone and takes what that run binds; a member
//! of the type the receiver is declared as binds in either run. A value a macro invocation writes binds no call.

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
/// [`kartta_lang::Conventions::own_type_receivers`], off that list only as its
/// [`kartta_lang::Conventions::unlisted_receivers`] say: nowhere, or here.
///
/// A call on a bare name that the language writes only for a class, of which the graph holds no type, binds neither
/// here nor at any tier below the include tiers.
pub const SAME_FILE: u8 = 80;
/// A definition in the caller's package directory binds, where a language's packages are directories.
pub const PACKAGE: u8 = 75;
/// The one file of those the caller's file includes that defines the name binds.
///
/// Where the language's includes are transitive and none of those defines the name, the one file of those the
/// includes reach in turn binds a name few definitions share. A method call of a method the language's standard library
/// declares, on a value the graph cannot type, binds here only where its receiver spells the callable's owner, and
/// otherwise at [`FIRST_PARTY`]: `src.as_ptr().cast()` binds the `Ptr::cast` of a module the file declares as a guess.
/// A method call binds at [`FIRST_PARTY`] too where another file the caller includes directly offers a type of the
/// name, which it cannot call: `hasher.hash()` binds the one `hash` member beside `eastl::hash` as a guess.
///
/// A bare use under [`ValueUses::Expanded`](kartta_lang::ValueUses::Expanded) that its file and includes leave open
/// binds here the definition of
/// [`Conventions::module_seen_everywhere`](kartta_lang::Conventions::module_seen_everywhere), unless
/// [`Conventions::native_names`](kartta_lang::Conventions::native_names) holds its name.
pub const INCLUDE: u8 = 70;
/// A re-export chain from an included file binds the definition it renames.
pub const RE_EXPORT: u8 = 65;
/// The one callable of the name binds from anywhere, and where the language's includes paste text, from a file that
/// reaches it or a declaration of it, or that a file reaching one includes directly.
///
/// A call of a method the language's runtime gives its values binds here only where its receiver spells that
/// callable's owner in any case: `session.seek()` binds the one `seek`, `Session.seek`. A method call of a method the
/// language's standard library declares, on a value the graph cannot type, binds here only where its receiver spells
/// the callable's owner, and otherwise at [`FIRST_PARTY`]: `found.start()` binds `Watcher::start` as a guess. A call
/// from outside test code binds no test definition here, and a test definition still counts among the definitions of
/// its name, except against the one type outside test code, which such a call binds.
///
/// A bare use under [`ValueUses::Expanded`](kartta_lang::ValueUses::Expanded) binds here the one definition the files
/// including its file bind the name to, or, where they bind none, the files a public `require` hands its file to.
///
/// A DSHL use its own file and includes leave unbound binds here the one file that declares the name, of all the
/// scripts the use's file compiles in.
///
/// Under [`Conventions::module_seen_everywhere`](kartta_lang::Conventions::module_seen_everywhere), a call with no
/// receiver binds here and at the steps below only a definition its file reaches or that module holds.
pub const UNIQUE: u8 = 60;
/// A definition a macro invocation writes binds at a tier this high at most, and none below [`SAME_DIR`].
///
/// Such a definition's name is often a trait's method or another file's macro-made namesake, which a call may mean
/// instead.
pub const MADE: u8 = 55;
/// The one callable of the name in the caller's directory binds.
///
/// Here and at the two steps after it, a call from outside test code binds no test definition, and a test definition
/// still counts among the definitions of its name nearest the caller.
pub const SAME_DIR: u8 = 50;
/// The one callable of the name in the caller's parent directory binds.
pub const PARENT_DIR: u8 = 30;
/// The one callable of the name outside vendored code binds.
///
/// The one callable of the name nearest the caller binds here too, at any step, where the call's arguments turned
/// another definition of the name away. An attribute write on a value binds here whatever tier found it, where the
/// language marks one by [`kartta_lang::Conventions::setter_suffix`], and so does a member of the caller's owner on a
/// field of a listed receiver, where the language's [`kartta_lang::Conventions::unlisted_receivers`] say so. A file
/// whose top level names a member, which keeps no receiver for the name, lists among the member's references here.
pub const FIRST_PARTY: u8 = 15;
