//! The arguments each tool takes. A field's doc comment is the description its JSON schema hands the agent, so it is
//! written for the agent and in its words: tool names, attribute names and example values as the agent types them.
#![expect(
    clippy::doc_markdown,
    clippy::doc_link_with_quotes,
    reason = "the field docs are JSON schema descriptions an agent reads, not rustdoc"
)]

use std::num::NonZeroU32;

use kartta::confidence;
use serde::Deserialize;

/// The least confidence a call needs to count, named for how the resolver bound it.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ConfidenceFloor {
    /// Keep every edge, the ones bound by the name alone included.
    #[default]
    Any,
    /// Keep the edges bound through the module layout (package, include, unique name, directory) and better.
    Likely,
    /// Keep only the edges bound through structure: a qualified callee, a receiver, an import, the caller's own file.
    Certain,
}

impl ConfidenceFloor {
    pub(crate) const fn floor(self) -> u8 {
        match self {
            Self::Any => 0,
            Self::Likely => confidence::SAME_DIR,
            Self::Certain => confidence::SAME_FILE,
        }
    }
}

/// A caller role a `find_callers` answer keeps: the `role=` a `<ref/>` carries.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CallerRole {
    /// Ordinary code: no test, entry point or generated code.
    Consumer,
    /// Test code.
    Test,
    /// A program entry point.
    Entry,
    /// Generated code.
    Generated,
}

impl CallerRole {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Consumer => "consumer",
            Self::Test => "test",
            Self::Entry => "entry",
            Self::Generated => "generated",
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexRepoParams {
    /// Absolute filesystem path of the repository root to index, e.g. "/home/me/projects/app". Comes from the user or
    /// the session context, not from a tool output.
    pub(crate) root: String,
    /// Optional allow-list of languages to build code graphs for. Valid names are the lowercase language names, NOT
    /// file extensions (e.g. `quirrel` not `nut`, `cpp` not `h`): go, cpp, typescript, tsx, javascript, daslang,
    /// quirrel, blk, rust, python, bash, php, ruby, lua, puppet, dshl - plus the family key jsts (typescript + tsx +
    /// javascript, which build as ONE graph and report as `jsts`). An unrecognized name is an error listing the
    /// accepted set, and index_stats reports the built graphs after a build. Omitted or null builds every language; an
    /// empty list builds no code graphs. Files of excluded languages are still indexed for the file tools (find_files,
    /// glob_files, grep, list_dir) - they just get no code graph. Use only when you know you will query one or two
    /// languages, e.g. ["go"].
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Whether to also index the files .gitignore hides - build output, vendored code, node_modules... Defaults to
    /// false, which applies every .gitignore inside the tree and is almost always what you want: their size reflects
    /// what the machine has built, not what the repository contains.
    #[serde(default)]
    pub(crate) index_ignored: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindFilesParams {
    /// Fuzzy pattern matched against repo-relative paths: characters must appear in order but gaps are fine, and
    /// filename matches rank above directory matches - "serv" finds pkg/service.go, "cfgyaml" finds
    /// internal/config/config.yaml. Whitespace separates terms that must all match. Your own search term; required
    /// and non-empty. To list files without a name in mind, use glob_files instead.
    pub(crate) pattern: String,
    /// Glob patterns; a path matching ANY of them stays in the candidate set, e.g. ["**/*.go", "lib/**"]. A glob
    /// without "/" matches across directories ("*.go" matches at any depth). Omit to search every indexed file.
    #[serde(default)]
    pub(crate) includes: Vec<String>,
    /// Glob patterns; a path matching ANY of them is dropped, e.g. ["**/vendor/**", "*_test.go"]. Omit to exclude
    /// nothing.
    #[serde(default)]
    pub(crate) excludes: Vec<String>,
    /// Maximum number of paths to return; 0 or omitted returns every match. Null is rejected; omit the argument for
    /// every match.
    #[serde(default)]
    pub(crate) max_results: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindCallersParams {
    /// The symbol whose callers to trace. A qualified name from a find_symbols hit (e.g. "pkg.New") resolves
    /// unambiguously; a name qualified by its owner ("Service.Greet") picks the callable it names; a bare name ("New")
    /// picks the unique match, or the most-called candidate when several share it, and the answer counts the others
    /// in passed_over.
    pub(crate) name: String,
    /// Optional language filter (multiselect). Omit to trace in every indexed language - a name resolving in several
    /// graphs yields one match per language; narrow to the language the symbol is defined in when you know it, e.g.
    /// ["go"]. Lowercase language names - the set index_repo / index_stats report; anything else is a tool error
    /// listing the accepted set. A valid name absent from this repo contributes no match. An empty list selects no
    /// language, so the answer is empty. Null is equivalent to omission.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Caller-hop limit from 1 through 4294967295: 1 = direct callers only. Omitted or null uses the default 3.
    /// Callers beyond this limit are not reached or counted; unexpanded= on a match counts the callers at the limit
    /// whose own callers were not followed.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) hops: Option<NonZeroU32>,
    /// Drop call edges the resolver is less sure about than this floor. "any" (the default) keeps every edge
    /// including name-only guesses. Null is rejected; omit the argument to use "any". "likely" keeps edges backed by
    /// module structure (same package, include, unique name, same directory) and better; "certain" keeps only
    /// structurally resolved edges (qualified, receiver, import, or same-file matches). Weak edges in answers are
    /// marked with the same vocabulary (confidence="likely"/"guess" on <ref/> records), and below_confidence= on a
    /// match counts the callers the floor dropped.
    #[serde(default)]
    pub(crate) min_confidence: ConfidenceFloor,
    /// Keep only the callers of this role: "consumer", "test", "entry", or "generated" - the role= value a <ref/>
    /// carries. Each <match> then counts its callers per role (consumers, tests, entries, generated), and count still
    /// counts every caller. The walk is the same with or without it, so a caller of another role still leads to the
    /// callers beyond it. Omitted or null keeps every caller.
    #[serde(default)]
    pub(crate) role: Option<CallerRole>,
}
