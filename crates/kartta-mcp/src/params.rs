//! The arguments each tool takes. A field's doc comment is the description its JSON schema hands the agent, so it is
//! written for the agent and in its words: tool names, attribute names and example values as the agent types them.
#![expect(
    clippy::doc_markdown,
    clippy::doc_link_with_quotes,
    reason = "the field docs are JSON schema descriptions an agent reads, not rustdoc"
)]

use std::borrow::Cow;
use std::num::{NonZeroU32, NonZeroUsize};

use kartta::{Access, confidence};
use rmcp::ErrorData;
use rmcp::handler::server::common::FromContextPart;
use rmcp::handler::server::tool::ToolCallContext;
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// What every tool but index_root and index_stats says of its `root`.
const ROOT: &str = "Absolute path of the repository root, as index_root was given it.";

/// What every symbol tool says of its `languages`.
const LANGUAGES: &str = "Graphs to search, by the language names index_stats reports, e.g. [\"go\"]; typescript, tsx \
    and javascript each search their one jsts graph. Omitted searches every language; an unknown name is an error \
    listing the names taken.";

/// What every call tool says of its `min_confidence`.
const MIN_CONFIDENCE: &str = "Least confidence a call or a use needs to count: \"likely\" drops the \
    confidence=\"guess\" ones, \"certain\" keeps only the unmarked. Omitted keeps all (\"any\").";

/// What a tool that picks one symbol of a name says of its `path_prefix`.
const PATH_PREFIX: &str = "Pick only among the symbols in files under this repo-relative path prefix, e.g. a \
    <candidate/>'s file.";

/// What a tool that picks one symbol of a name says of its `line`.
const LINE: &str = "Pick only among the symbols that start on this 1-based line, e.g. a <candidate/>'s loc line; with \
    path_prefix it reaches one of several namesakes in one file.";

/// What a tool that takes one file says of it.
const FILE: &str = "Repo-relative path with forward slashes, e.g. \"pkg/service.go\".";

/// What `find_files` and `glob_files` say of their `max_results`.
const MAX_PATHS: &str = "Most paths listed; default 200, and 0 lists every match.";

/// What `find_callers` and `find_callees` say of the `name` they walk from.
const CALLABLE: &str = "The function or method: a qualified name from a find_symbols hit (\"pkg.New\"), one qualified \
    by its owner with its language's separator (\"Service.Greet\", \"Service::greet\"), or a bare name. A name that \
    selects nothing is read again with \".\" and \"::\" swapped, named in read_as=. Among several, the pick prefers an \
    exact qualified-name match, then a callable, then the most called, then path order.";

/// A tool's arguments, read as `P`. A wrong argument fails the call with a tool error that names the parameter by its
/// path inside the arguments, such as `regions[0].start_line`, then says what the parameter takes.
// constraint: rmcp's `#[tool]` macro takes a tool's input schema from the argument whose type is named `Parameters`, and
// its router answers an extraction error as a tool error only when the message opens with "failed to deserialize
// parameters:", else as a protocol error
#[derive(Debug)]
pub(crate) struct Parameters<P>(pub(crate) P);

impl<P: schemars::JsonSchema> schemars::JsonSchema for Parameters<P> {
    fn schema_name() -> Cow<'static, str> {
        P::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        P::json_schema(generator)
    }
}

impl<S, P: DeserializeOwned> FromContextPart<ToolCallContext<'_, S>> for Parameters<P> {
    fn from_context_part(context: &mut ToolCallContext<'_, S>) -> Result<Self, ErrorData> {
        let arguments = context.arguments.take().unwrap_or_default();
        serde_path_to_error::deserialize(serde_json::Value::Object(arguments))
            .map(Self)
            .map_err(|error| {
                ErrorData::invalid_params(
                    format!("failed to deserialize parameters: {error}"),
                    None,
                )
            })
    }
}

/// The least confidence a call needs to count, named for how the resolver bound it.
#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ConfidenceFloor {
    #[default]
    Any,
    Likely,
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
    Consumer,
    Test,
    Entry,
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
pub(crate) struct IndexRootParams {
    /// Absolute path of the repository root to index, from the user or the session, e.g. "/home/me/projects/app".
    pub(crate) root: String,
    /// Languages to build code graphs for, by lowercase name, not extension: go, cpp, typescript, tsx, javascript,
    /// daslang, quirrel, blk, rust, python, bash, php, ruby, lua, puppet, dshl, and jsts for typescript, tsx and
    /// javascript, which build one graph. Omitted builds every language. A language left out keeps its files for the
    /// file tools. Use it only to query one or two languages, e.g. ["go"].
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Also index the files .gitignore hides (build output, vendored code, node_modules); default false, which applies
    /// every .gitignore inside the tree and none above the root.
    #[serde(default)]
    pub(crate) index_ignored: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexStatsParams {
    /// Absolute path of the repository root to report, as index_root was given it; omitted lists every root the server
    /// holds.
    #[serde(default)]
    pub(crate) root: Option<String>,
    /// Also resolve every call site and add a <coverage/> record per language; it takes about half a second on a large
    /// repository each time.
    #[serde(default)]
    pub(crate) coverage: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RefreshParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// Repo-relative paths of the changed files or directories, as find_files prints them; a directory covers
    /// everything below it, and a deleted or renamed file is named by its old path and its new one. An absolute path, a
    /// drive, a backslash or a .. name refuses the call. Omitted checks the whole tree.
    #[serde(default)]
    pub(crate) paths: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindFilesParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// Fuzzy pattern matched against repo-relative paths: characters in order, gaps allowed ("cfgyaml" finds
    /// internal/config/config.yaml); whitespace separates terms that must all match.
    pub(crate) pattern: String,
    /// Glob patterns; a path matching any stays a candidate, e.g. ["**/*.go", "lib/**"]; a glob without / matches at
    /// any depth.
    #[serde(default)]
    pub(crate) includes: Option<Vec<String>>,
    /// Glob patterns; a matching path is dropped, and a matching directory with all beneath it, e.g. ["**/vendor",
    /// "*_test.go"].
    #[serde(default)]
    pub(crate) excludes: Option<Vec<String>>,
    #[schemars(description = MAX_PATHS)]
    #[serde(default)]
    pub(crate) max_results: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindCallersParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    #[schemars(description = CALLABLE)]
    pub(crate) name: String,
    #[serde(default)]
    #[schemars(description = PATH_PREFIX)]
    pub(crate) path_prefix: Option<String>,
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    #[schemars(description = LINE)]
    pub(crate) line: Option<NonZeroU32>,
    #[serde(default)]
    #[schemars(description = LANGUAGES)]
    pub(crate) languages: Option<Vec<String>>,
    /// Caller hops walked, 1 for direct callers only; default 3.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) hops: Option<NonZeroU32>,
    #[serde(default)]
    #[schemars(description = MIN_CONFIDENCE)]
    pub(crate) min_confidence: Option<ConfidenceFloor>,
    /// Keep only the callers of this role= value; the walk and every count stay the same.
    #[serde(default)]
    pub(crate) role: Option<CallerRole>,
    /// Most <ref/> records a match lists, nearest first, after role keeps its callers; default 100.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindCalleesParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    #[schemars(description = CALLABLE)]
    pub(crate) name: String,
    #[serde(default)]
    #[schemars(description = PATH_PREFIX)]
    pub(crate) path_prefix: Option<String>,
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    #[schemars(description = LINE)]
    pub(crate) line: Option<NonZeroU32>,
    #[serde(default)]
    #[schemars(description = LANGUAGES)]
    pub(crate) languages: Option<Vec<String>>,
    /// Callee hops walked, 1 for direct callees only; default 1. Each hop multiplies the answer, so raise it only to
    /// follow a short chain.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) hops: Option<NonZeroU32>,
    #[serde(default)]
    #[schemars(description = MIN_CONFIDENCE)]
    pub(crate) min_confidence: Option<ConfidenceFloor>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangedRegion {
    #[schemars(description = FILE)]
    pub(crate) file: String,
    /// First changed line, 1-based; default 1.
    #[serde(default)]
    pub(crate) start_line: Option<NonZeroU32>,
    /// Last changed line, 1-based inclusive; omitted reaches the end of the file.
    #[serde(default)]
    pub(crate) end_line: Option<NonZeroU32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImpactOfChangeParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// The changed line ranges, one per hunk, e.g. [{"file": "pkg/a.go", "start_line": 10, "end_line": 24}, {"file":
    /// "pkg/b.go"}]; a region naming only its file covers the whole file. A symbol is changed when its definition
    /// overlaps a region.
    #[serde(default)]
    pub(crate) regions: Option<Vec<ChangedRegion>>,
    /// A unified diff as git diff prints it (-U0 keeps it short), hunk bodies included: the lines of each hunk's + side
    /// are a region of its +++ file, a hunk that only deletes covers the line on each side of the deletion, and a
    /// deleted file is a whole-file region. The + side is the tree with the change applied, so index that tree. A path
    /// git writes quoted is not decoded: pass that file as a region.
    #[serde(default)]
    pub(crate) diff: Option<String>,
    /// Most changed symbols listed, and apart from them most hotspots; default 20.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindUnusedParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    #[serde(default)]
    #[schemars(description = LANGUAGES)]
    pub(crate) languages: Option<Vec<String>>,
    /// Judge only the definitions in files under this repo-relative path prefix, e.g. "pkg/billing/" (a trailing /
    /// takes that directory alone) or one file. Omitted judges the whole repository, which takes seconds on a large
    /// one.
    #[serde(default)]
    pub(crate) path_prefix: Option<String>,
    /// Most definitions listed; default 50.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobFilesParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// Glob patterns; a path matching any is listed, e.g. ["**/*.go", "src/**/*.ts"]; a glob without / matches at any
    /// depth. Pass ["**"] for the whole tree.
    pub(crate) globs: Vec<String>,
    /// Glob patterns; a matching path is dropped, and a matching directory with all beneath it, e.g. ["**/vendor"].
    #[serde(default)]
    pub(crate) excludes: Option<Vec<String>>,
    #[schemars(description = MAX_PATHS)]
    #[serde(default)]
    pub(crate) max_results: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrepParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// Rust regular expressions matched against single lines; a line matching any is listed, so pass naming variants
    /// together, e.g. ["FooBar", "foo_bar"]. No lookarounds or backreferences; escape literal braces, as
    /// "interface\\{\\}".
    pub(crate) patterns: Vec<String>,
    /// Glob patterns; a path matching any is searched: "dir/**" for a subtree, a file's exact path, "*.go" at any
    /// depth. Omitted searches every indexed file.
    #[serde(default)]
    pub(crate) includes: Option<Vec<String>>,
    /// Glob patterns; a matching path is dropped, and a matching directory with all beneath it. Omitted on a search
    /// without includes, it drops **/vendor, **/node_modules, **/dist and **/__pycache__; with includes, nothing. Pass
    /// [] to search everything.
    #[serde(default)]
    pub(crate) excludes: Option<Vec<String>>,
    /// Fold case when true; default false.
    #[serde(default)]
    pub(crate) case_insensitive: Option<bool>,
    /// Lines of context a side of each match, like grep -C; default 0. They count toward no cap; past 40,000 bytes of
    /// lines, each match keeps the context that fits, which context_shown= counts.
    #[serde(default)]
    pub(crate) context_lines: Option<usize>,
    /// Most match lines listed across all files, in path order; default 100.
    #[serde(default)]
    pub(crate) max_results: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListDirParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// Repo-relative directory with forward slashes, e.g. "src/web"; omitted, "" or "." lists the root.
    #[serde(default)]
    pub(crate) path: Option<String>,
    /// Directory levels listed; default 1, the immediate children.
    #[serde(default)]
    pub(crate) max_depth: Option<NonZeroUsize>,
    /// Most entries listed across all levels; default 1000.
    #[serde(default)]
    pub(crate) max_entries: Option<NonZeroUsize>,
    /// Directory names matched literally at any depth ("vendor", "pkg/deep"), never globs: the listing does not descend
    /// into them. Omitted, it skips vendor, node_modules, dist, build and __pycache__; pass [] to descend everywhere.
    /// The directory named in path is listed in full.
    #[serde(default)]
    pub(crate) excludes: Option<Vec<String>>,
    /// Leave out the entries whose name starts with ".", counted in excluded=; default false.
    #[serde(default)]
    pub(crate) skip_dotfiles: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindSymbolsParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// Keywords matched against the words of symbol names and of their files' paths, e.g. "BuildConfig" or "parse
    /// config"; camelCase and snake_case split into words, and whole words match.
    pub(crate) query: String,
    #[serde(default)]
    #[schemars(description = LANGUAGES)]
    pub(crate) languages: Option<Vec<String>>,
    /// Most hits across all searched languages; default 20.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindCallPathParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// The symbol the chain starts at: a qualified name from a find_symbols hit, one qualified by its owner with its
    /// language's separator, or a bare name, read again with "." and "::" swapped when it selects nothing
    /// (from_read_as=). Among several, the pick prefers an exact qualified-name match, then a callable, then the most
    /// called, then path order.
    pub(crate) from: String,
    /// Pick from only among the symbols in files under this repo-relative path prefix, e.g. a <candidate end="from"/>'s
    /// file.
    #[serde(default)]
    pub(crate) from_path_prefix: Option<String>,
    /// Pick from only among the symbols that start on this 1-based line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) from_line: Option<NonZeroU32>,
    /// The symbol the chain ends at, named, read (to_read_as=) and picked as from is.
    pub(crate) to: String,
    /// Pick to only among the symbols in files under this repo-relative path prefix, e.g. a <candidate end="to"/>'s
    /// file.
    #[serde(default)]
    pub(crate) to_path_prefix: Option<String>,
    /// Pick to only among the symbols that start on this 1-based line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) to_line: Option<NonZeroU32>,
    #[serde(default)]
    #[schemars(description = LANGUAGES)]
    pub(crate) languages: Option<Vec<String>>,
    /// Longest chain searched, in calls; default 8.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) hops: Option<NonZeroU32>,
    #[serde(default)]
    #[schemars(description = MIN_CONFIDENCE)]
    pub(crate) min_confidence: Option<ConfidenceFloor>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindReferencesParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// The symbol: a qualified name from a find_symbols hit ("pkg.Config"), one qualified by its owner with its
    /// language's separator ("Config.Timeout", "Config::timeout"), or a bare name. A name that selects nothing is read
    /// again with "." and "::" swapped, named in read_as=. Among several, the pick prefers an exact qualified-name
    /// match, then the most recorded sites, then path order.
    pub(crate) name: String,
    #[serde(default)]
    #[schemars(description = PATH_PREFIX)]
    pub(crate) path_prefix: Option<String>,
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    #[schemars(description = LINE)]
    pub(crate) line: Option<NonZeroU32>,
    /// List only the sites and file_scope files under this repo-relative path prefix, e.g. the directory of the files
    /// past those a match names. It narrows what is listed; path_prefix narrows the symbol picked.
    #[serde(default)]
    pub(crate) sites_under: Option<String>,
    #[serde(default)]
    #[schemars(description = LANGUAGES)]
    pub(crate) languages: Option<Vec<String>>,
    #[serde(default)]
    #[schemars(description = MIN_CONFIDENCE)]
    pub(crate) min_confidence: Option<ConfidenceFloor>,
    /// Keep only the uses that read the symbol ("read"), set it ("write") or name it as a type ("type"), e.g. "write"
    /// for "who sets Config.Timeout?". An update (+=, ++) is a read and a write, a use as a type is also a read, and
    /// calls are dropped. The match's counts still count every site.
    #[serde(default)]
    pub(crate) access: Option<AccessFilter>,
}

/// Which uses a `find_references` answer keeps.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AccessFilter {
    Read,
    Write,
    Type,
}

impl AccessFilter {
    pub(crate) const fn access(self) -> Access {
        match self {
            Self::Read => Access::Read,
            Self::Write => Access::Write,
            Self::Type => Access::Type,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadDefinitionsParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// 1 to 10 names, each bare ("New") or qualified by its package or owner with its language's separator ("pkg.New",
    /// "Service::greet"). A qualified name selects the definitions whose qualified name equals it or ends with it at a
    /// separator, and those whose qualified name it ends with ("ns::Service::Greet" selects "Service::Greet"). A name
    /// that selects nothing is read again with "." and "::" swapped, named in read_as=.
    #[schemars(length(min = 1, max = 10), inner(length(min = 1)))]
    pub(crate) names: Vec<String>,
    /// Read only the definitions in files under this repo-relative path prefix, for every name, e.g. a <candidate/>'s
    /// file.
    #[serde(default)]
    pub(crate) path_prefix: Option<String>,
    /// Read only the definitions that start on this 1-based line, e.g. a <candidate/>'s loc line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) line: Option<NonZeroU32>,
    /// Repo-relative path of the file the names are read for: the definitions in its directory rank first.
    #[serde(default)]
    pub(crate) from: Option<String>,
    #[serde(default)]
    #[schemars(description = LANGUAGES)]
    pub(crate) languages: Option<Vec<String>>,
    /// Most definitions read per name; default 3.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindImplementationsParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    /// The interface, base class, trait, mixed-in module or type alias, bare or qualified with its language's separator
    /// ("io.Reader", "ui::Widget"); a name that selects nothing is read again with "." and "::" swapped, named in
    /// read_as=. Among several types of the name, the pick prefers an exact qualified-name match, then the most
    /// implemented. A type alias answers the types whose parent names the alias, not its target.
    pub(crate) name: String,
    #[serde(default)]
    #[schemars(description = PATH_PREFIX)]
    pub(crate) path_prefix: Option<String>,
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    #[schemars(description = LINE)]
    pub(crate) line: Option<NonZeroU32>,
    #[serde(default)]
    #[schemars(description = LANGUAGES)]
    pub(crate) languages: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileOutlineParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    #[schemars(description = FILE)]
    pub(crate) file: String,
    /// Most <symbol/> records listed, in source order; default 100.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindImportersParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    #[schemars(description = FILE)]
    pub(crate) file: String,
    /// Import hops followed, 1 for direct importers only; default 3.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) hops: Option<NonZeroU32>,
    /// Most <file/> records listed, nearest first; default 200.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImpactOfEditParams {
    #[schemars(description = ROOT)]
    pub(crate) root: String,
    #[schemars(description = FILE)]
    pub(crate) file: String,
    /// First line of the span, 1-based, e.g. a diff hunk's first changed line.
    pub(crate) start_line: NonZeroU32,
    /// Last line of the span, 1-based inclusive; omitted reaches the end of the file. A symbol is in the span when its
    /// definition overlaps it.
    #[serde(default)]
    pub(crate) end_line: Option<NonZeroU32>,
    /// Most <symbol> records listed, the widest reach kept in source order, and apart from them most files each of
    /// <includes> and <included-by> lists; default 10.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}
