//! An MCP server over the kartta engine: the file index and code graph of each repository root it indexes, answered
//! to an agent as tools.
//!
//! The server speaks MCP over stdio and offers tools alone, no resources and no prompts. `index_root` builds the
//! engine over a repository root, and every other tool answers from the root it names: the file tools search paths,
//! list directories and grep content, and the graph tools find definitions, callers, references, call paths,
//! implementors, importers and what an edit reaches. The process holds the engines of up to [`Config::max_roots`]
//! roots and builds one at a time: an `index_root` of another root keeps the others, and past that bound drops a
//! root whose build failed, else the least recently asked root that is not building. A built index
//! follows its tree through the operating system's change notifications and refreshes itself from each change, unless
//! [`Config::watch`] is off; the `refresh` tool takes in a change the notifications do not report.
//!
//! An answer is an XML-like envelope written for a model rather than a parser: one root element named for the
//! question, counts as attributes, one record per line. An attribute that marks the unusual, `truncated="true"` or
//! `confidence="guess"`, appears only where it applies, and an answer a cap, a depth bound or a filter cut says how
//! much it left out wherever the engine counts it. A call made wrong answers an MCP tool error naming the fix; a valid
//! call over an empty reality answers a successful envelope with a count of zero and a hint naming the next tool.
//!
//! [`Server::with_config`] takes the build's options from a [`Config`], and [`Config::from_env`] reads them from the
//! environment the `kartta-mcp` binary runs in. [`Server::index_at_launch`] starts a build without waiting for an
//! `index_root` call, and until it finishes every query answers that the index is still building.
#![expect(
    clippy::multiple_crate_versions,
    reason = "the MCP SDK's macros build on syn 3 while serde's derive and the grammars' builds are on syn 2"
)]

mod config;
mod diff;
mod languages;
mod llmxml;
mod params;
mod paths;
mod render;
mod server;
mod state;

pub use crate::config::Config;
pub use crate::server::Server;
