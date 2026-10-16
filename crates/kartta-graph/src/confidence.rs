//! How sure each tier of the call cascade is of the symbol it binds, from 95 down to 15.
//!
//! A call or a use is resolved by the first tier that binds it, and the answer carries that tier's confidence: a
//! [`Limits`](crate::Limits) floor of [`INCLUDE`] keeps the calls bound through an include and every tier above it.

/// A receiver that names an import binds an export of the imported directory, or a dot import binds its one export of
/// the name.
pub const IMPORT: u8 = 95;
/// A callee written qualified binds the symbol of that qualified name.
pub const QUALIFIED: u8 = 90;
/// A callee written with more scopes than a qualified name keeps binds its trailing owner and name.
pub const QUALIFIED_TAIL: u8 = 88;
/// A receiver that names an owner binds that owner's member.
pub const RECEIVER: u8 = 85;
/// A local, or a definition in the caller's own file, binds; so does a member of the caller's owner when the receiver
/// names no owner.
pub const SAME_FILE: u8 = 80;
/// A definition in the caller's package directory binds, where a language's packages are directories.
pub const PACKAGE: u8 = 75;
/// The one file of those the caller's file includes that defines the name binds.
pub const INCLUDE: u8 = 70;
/// A re-export chain from an included file binds the definition it renames.
pub const RE_EXPORT: u8 = 65;
/// The one callable of the name binds from anywhere.
pub const UNIQUE: u8 = 60;
/// The one callable of the name in the caller's directory binds.
pub const SAME_DIR: u8 = 50;
/// The one callable of the name in the caller's parent directory binds.
pub const PARENT_DIR: u8 = 30;
/// The one callable of the name outside vendored code binds.
pub const FIRST_PARTY: u8 = 15;
