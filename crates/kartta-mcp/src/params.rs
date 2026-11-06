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
    /// false, which applies every .gitignore inside the tree, and none in a directory above the root, and is almost
    /// always what you want: their size reflects what the machine has built, not what the repository contains.
    #[serde(default)]
    pub(crate) index_ignored: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexStatsParams {
    /// When true, also resolve every call site of each language's graph and add a <coverage/> record per language:
    /// how many sites bound a definition and how surely, and why the rest bound none. It resolves every call in the
    /// repository, about half a second on a large one, each time it is asked. Omitted or false answers at once
    /// without it.
    #[serde(default)]
    pub(crate) coverage: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RefreshParams {
    /// Repo-relative paths with forward slashes of the files or directories that changed, as find_files prints them,
    /// never absolute, with a drive such as C:, a backslash or a .. name; a directory covers everything below it, and a
    /// deleted or renamed file is named by its old path as well as its new one. Omit to check the whole tree.
    #[serde(default)]
    pub(crate) paths: Option<Vec<String>>,
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
    pub(crate) includes: Option<Vec<String>>,
    /// Glob patterns; a path matching ANY of them is dropped, and a matching directory with everything beneath it,
    /// e.g. ["**/vendor", "*_test.go"]. Omit to exclude nothing.
    #[serde(default)]
    pub(crate) excludes: Option<Vec<String>>,
    /// Maximum number of paths to return; 0, omitted or null returns every match.
    #[serde(default)]
    pub(crate) max_results: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindCallersParams {
    /// The symbol whose callers to trace. A qualified name from a find_symbols hit (e.g. "pkg.New") picks the symbol
    /// of that name; a name qualified by its owner with its language's separator ("Service.Greet" in Go, Python or
    /// TypeScript, "Service::greet" in Rust or C++) picks the callable it names; a bare name ("New") picks the unique
    /// match. A name that selects nothing as written is read with each "." as "::", or each "::" as ".", and read_as=
    /// names that reading. Where several share the name - a bare name, or overloads of one qualified name - the
    /// most-called candidate is picked, the answer counts the others in passed_over and lists the most called of them
    /// as <candidate/> records; check the match's loc, and pass a candidate's file as path_prefix and its line as line
    /// to pick it. A blank name is a tool error.
    pub(crate) name: String,
    /// Repo-relative path prefix with forward slashes: only the symbols in files whose path starts with it are picked
    /// from, e.g. a <candidate/>'s file, or "vendor/" for the vendored copy of a package. It is how to reach one of
    /// several copies that share a qualified name. Omitted picks from every file.
    #[serde(default)]
    pub(crate) path_prefix: Option<String>,
    /// 1-based line the symbol starts on, e.g. a <candidate/>'s loc line: only the symbols that start on it are
    /// picked from. With path_prefix set to the loc's file, it is how to reach one of several symbols of the name in
    /// one file - C++ overloads, a declaration beside its definition. Omitted or null picks from every line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) line: Option<NonZeroU32>,
    /// Optional language filter (multiselect). Omit to trace in every indexed language - a name resolving in several
    /// graphs yields one match per language; narrow to the language the symbol is defined in when you know it, e.g.
    /// ["go"]. Lowercase language names - the set index_repo / index_stats report; anything else is a tool error
    /// listing the accepted set. A valid name absent from this repo contributes no match. An empty list selects no
    /// language, so the answer is empty. Null is equivalent to omission.
    /// A language of a family filters as the whole family, as typescript, tsx and javascript each search the jsts
    /// graph, and the answer's languages= names the families searched.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Caller-hop limit from 1 through 4294967295: 1 = direct callers only. Omitted or null uses the default 3.
    /// Callers beyond this limit are not reached or counted; unexpanded= on a match counts the callers at the limit
    /// whose own callers were not followed.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) hops: Option<NonZeroU32>,
    /// Drop call edges the resolver is less sure about than this floor. "any" (the default) keeps every edge
    /// including name-only guesses. "likely" keeps edges backed by
    /// module structure (same package, include, unique name, same directory) and better; "certain" keeps only
    /// structurally resolved edges (qualified, receiver, import, or same-file matches). Weak edges in answers are
    /// marked with the same vocabulary (confidence="likely"/"guess" on <ref/> records), and below_confidence= on a
    /// match counts the callers the floor dropped.
    #[serde(default)]
    pub(crate) min_confidence: Option<ConfidenceFloor>,
    /// Keep only the callers of this role: "consumer", "test", "entry", or "generated" - the role= value a <ref/>
    /// carries. Each <match> then counts its callers per role (consumers, tests, entries, generated), and count still
    /// counts every caller. The walk is the same with or without it, so a caller of another role still leads to the
    /// callers beyond it. Omitted or null keeps every caller.
    #[serde(default)]
    pub(crate) role: Option<CallerRole>,
    /// Positive cap on the <ref/> records a match lists, nearest first, after 'role' keeps its callers. Omitted or null
    /// = 100. Zero and negatives are rejected. count= and the reach band still count every caller the walk reached,
    /// and shown= on a match says how many refs it lists when the cap cut them.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindCalleesParams {
    /// The symbol whose callees to trace. A qualified name from a find_symbols hit (e.g. "pkg.New") picks the symbol
    /// of that name; a name qualified by its owner with its language's separator ("Service.Greet" in Go, Python or
    /// TypeScript, "Service::greet" in Rust or C++) picks the callable it names; a bare name ("New") picks the unique
    /// match. A name that selects nothing as written is read with each "." as "::", or each "::" as ".", and read_as=
    /// names that reading. Where several share the name - a bare name, or overloads of one qualified name - the
    /// most-called candidate is picked, the answer counts the others in passed_over and lists the most called of them
    /// as <candidate/> records; check the match's loc, and pass a candidate's file as path_prefix and its line as line
    /// to pick it. A blank name is a tool error.
    pub(crate) name: String,
    /// Repo-relative path prefix with forward slashes: only the symbols in files whose path starts with it are picked
    /// from, e.g. a <candidate/>'s file. Omitted picks from every file.
    #[serde(default)]
    pub(crate) path_prefix: Option<String>,
    /// 1-based line the symbol starts on, e.g. a <candidate/>'s loc line: only the symbols that start on it are
    /// picked from. With path_prefix set to the loc's file, it is how to reach one of several symbols of the name in
    /// one file - C++ overloads, a declaration beside its definition. Omitted or null picks from every line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) line: Option<NonZeroU32>,
    /// Optional language filter (multiselect). Omit to trace in every indexed language - a name resolving in several
    /// graphs yields one match per language; narrow to the language the symbol is defined in when you know it, e.g.
    /// ["go"]. Lowercase language names - the set index_repo / index_stats report; anything else is a tool error
    /// listing the accepted set. A valid name absent from this repo contributes no match. An empty list selects no
    /// language, so the answer is empty. Null is equivalent to omission.
    /// A language of a family filters as the whole family, as typescript, tsx and javascript each search the jsts
    /// graph, and the answer's languages= names the families searched.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Callee-hop limit from 1 through 4294967295: 1 = direct callees only. Omitted or null uses the default 1. Each
    /// hop multiplies the answer, so raise it only to follow a short chain. unexpanded= on a match counts the callees
    /// at the limit whose own callees were not followed.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) hops: Option<NonZeroU32>,
    /// Drop call edges the resolver is less sure about than this floor. "any" (the default) keeps every edge
    /// including name-only guesses. "likely" keeps edges backed by
    /// module structure (same package, include, unique name, same directory) and better; "certain" keeps only
    /// structurally resolved edges (qualified, receiver, import, or same-file matches). Weak edges in answers are
    /// marked with the same vocabulary (confidence="likely"/"guess" on <ref/> records), and below_confidence= on a
    /// match counts the callees the floor dropped.
    #[serde(default)]
    pub(crate) min_confidence: Option<ConfidenceFloor>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangedRegion {
    /// Repo-relative path with forward slashes, exactly as returned by find_files or find_symbols (e.g.
    /// "pkg/service.go") - not absolute, not a bare filename.
    pub(crate) file: String,
    /// First changed line, 1-based inclusive - e.g. the first line of a diff hunk's + side. Omitted or null = 1.
    #[serde(default)]
    pub(crate) start_line: Option<NonZeroU32>,
    /// Last changed line, 1-based inclusive. Omitted or null = the end of the file; a value before start_line is a
    /// tool error.
    #[serde(default)]
    pub(crate) end_line: Option<NonZeroU32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImpactOfChangeParams {
    /// The changed line ranges, one per diff hunk, e.g. [{"file": "pkg/a.go", "start_line": 10, "end_line": 24},
    /// {"file": "pkg/b.go"}]. A region naming only its file covers the whole file - a file the change adds or
    /// rewrites. A symbol counts as changed when its definition overlaps a region at all. Pass regions, diff, or both.
    #[serde(default)]
    pub(crate) regions: Option<Vec<ChangedRegion>>,
    /// A unified diff exactly as git diff prints it (git diff -U0 keeps it short), hunk bodies included: each hunk's
    /// lines on its + side become a region of the file its +++ b/ header names, a hunk that only deletes covers the
    /// line on each side of the deletion, and a deleted file (+++ /dev/null) is a whole-file region of its --- a/ path.
    /// The + side is the tree with the change applied, so index that tree; a deleted file answers only while the index
    /// still holds it. A path git writes quoted (a control or non-ASCII character) is not decoded and counts in
    /// unheld_regions and missing_regions - pass that file as a region. A diff holding no hunk is a tool error, and so is a hunk that does
    /// not read, the error naming its line.
    #[serde(default)]
    pub(crate) diff: Option<String>,
    /// Positive cap on the changed symbols listed and, separately, on the hotspots listed. Omitted or null = 20. Zero
    /// and negatives are rejected. The counts on the answer cover every one of them.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindUnusedParams {
    /// Optional language filter (multiselect). Omit to judge every indexed language; narrow when you clean up one,
    /// e.g. ["go"]. Lowercase language names - the set index_repo / index_stats report; anything else is a tool error
    /// listing the accepted set. A valid name absent from this repo contributes nothing. An empty list selects no
    /// language, so the answer is empty. Null is equivalent to omission.
    /// A language of a family filters as the whole family, as typescript, tsx and javascript each search the jsts
    /// graph, and the answer's languages= names the families searched.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Repo-relative path prefix with forward slashes: only definitions in files whose path starts with it are judged
    /// and counted, e.g. "pkg/billing/" (end it with "/" to take that directory alone) or one file's path. Omitted
    /// judges the whole repository, which takes seconds on a large one.
    #[serde(default)]
    pub(crate) path_prefix: Option<String>,
    /// Positive cap on the definitions listed. Omitted or null = 50. Zero and negatives are rejected. total= counts
    /// every unused definition found whatever the cap.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobFilesParams {
    /// Glob patterns; a path matching ANY of them is listed, e.g. ["**/*.go", "src/**/*.ts"]. A glob without "/"
    /// matches across directories ("*.go" matches at any depth). REQUIRED and non-empty - to enumerate the whole tree,
    /// pass ["**"]. To hunt ONE file by name, use find_files instead.
    pub(crate) globs: Vec<String>,
    /// Glob patterns; a path matching ANY of them is dropped, and a matching directory with everything beneath it,
    /// e.g. ["**/vendor"]. Omit to exclude nothing.
    #[serde(default)]
    pub(crate) excludes: Option<Vec<String>>,
    /// Maximum number of paths to return; 0, omitted or null returns every match. A list the cap cut ends with a line
    /// saying so.
    #[serde(default)]
    pub(crate) max_results: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrepParams {
    /// Rust regular expressions, each matched against single lines; a line is returned when ANY pattern matches
    /// (union) - pass naming variants in one call, e.g. ["FooBar", "foo_bar"]. No lookarounds or backreferences;
    /// matching never crosses lines. Literal braces need escaping: "interface\\{\\}". At least one non-blank pattern
    /// is required.
    pub(crate) patterns: Vec<String>,
    /// Glob patterns; a path matching ANY of them stays in the candidate set. Scope to a subtree with "dir/**" or to a
    /// single file with its exact path; "*.go" (no slash) matches at any depth. Omit to search every indexed file.
    #[serde(default)]
    pub(crate) includes: Option<Vec<String>>,
    /// Glob patterns; a matching path is dropped, and a matching directory with everything beneath it. OMITTED on an
    /// UNSCOPED search (no includes) defaults to vendored and generated trees (**/vendor, **/node_modules, **/dist,
    /// **/__pycache__); when 'includes' is set, omitted means no excludes - the scope is searched exactly as given.
    /// Pass [] to search everything on a broad sweep, or your own list to replace the default. excluded= on the answer
    /// counts the files the excludes dropped.
    #[serde(default)]
    pub(crate) excludes: Option<Vec<String>>,
    /// Fold case when true. Omitted = false (exact case).
    #[serde(default)]
    pub(crate) case_insensitive: Option<bool>,
    /// Lines of context around each match, like grep -C. Omitted = 0 (matches only). Context lines do not count
    /// toward max_results.
    #[serde(default)]
    pub(crate) context_lines: Option<usize>,
    /// Positive cap on match lines across all files, spent in file-path order. Omitted or null = 100. Zero and
    /// negatives are rejected. Once the cap fills, the search reads on until one more match shows the cap cut the
    /// answer, which then carries truncated="true"; unsearched= counts the files the answer does not cover from then
    /// on, or once a cancel stopped the search.
    #[serde(default)]
    pub(crate) max_results: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListDirParams {
    /// Repo-relative directory path with forward slashes, e.g. "src/web" - the directory part of any path returned by
    /// find_files or glob_files. "" or "." (or omitting it) lists the repository root.
    #[serde(default)]
    pub(crate) path: Option<String>,
    /// Positive number of directory levels to list. Omitted or null = 1: immediate children only. 2 adds their
    /// children. Zero and negatives are rejected. A directory the listing did not enter carries unlisted= with the
    /// count of entries beneath it.
    #[serde(default)]
    pub(crate) max_depth: Option<NonZeroUsize>,
    /// Positive cap on entries across all levels, spent in listing order. Omitted or null = 1000. Zero and negatives
    /// are rejected. The answer carries truncated="true" when the cap cut it.
    #[serde(default)]
    pub(crate) max_entries: Option<NonZeroUsize>,
    /// Directory NAMES matched literally at any depth ("vendor", or "pkg/deep" for a deep under a pkg), never globs: a
    /// name holding * ? [ ] { } or \ fails the call unless it names an indexed directory. A matched directory's
    /// contents are not descended into; the directory entry itself still appears, with unlisted= counting what it
    /// holds. OMITTED defaults to vendored and generated trees (vendor, node_modules, dist, build, __pycache__). Pass []
    /// to descend everywhere, or your own list to replace the default. The directory named in 'path' is always listed
    /// in full.
    #[serde(default)]
    pub(crate) excludes: Option<Vec<String>>,
    /// When true, entries whose name starts with "." are omitted entirely, and excluded= counts them. Omitted = false
    /// (dot entries are shown).
    #[serde(default)]
    pub(crate) skip_dotfiles: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindSymbolsParams {
    /// Keyword(s) matched against the words of symbol names and of their files' paths, e.g. "BuildConfig" or "parse
    /// config"; camelCase and snake_case split into words. A bare identifier works best; partial names match. A blank
    /// query is a tool error.
    pub(crate) query: String,
    /// Optional language filter (multiselect). Omit to search every indexed language; name one or more to narrow when
    /// you know where the symbol lives, e.g. ["go"] or ["jsts"]. Lowercase language names - the set
    /// index_repo / index_stats report; anything else is a tool error listing the accepted set. A valid name absent
    /// from this repo just contributes no hits. An empty list selects no language, so the answer is empty. Null is
    /// equivalent to omission. A language of a family filters as the whole family, as typescript, tsx and javascript
    /// each search the jsts graph, and the answer's languages= names the families searched.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Positive maximum hits across all searched languages. Omitted or null uses the default 20. Zero and negatives
    /// are rejected.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindCallPathParams {
    /// The symbol the chain starts at: the first caller. A qualified name from a find_symbols hit (e.g. "pkg.Serve")
    /// picks the symbol of that name; a name qualified by its owner with its language's separator ("Service.Greet" in
    /// Go, Python or TypeScript, "Service::greet" in Rust or C++) picks the callable it names; a bare name picks the
    /// unique match. A name that selects nothing as written is read with each "." as "::", or each "::" as ".", and
    /// from_read_as= names that reading. Where several share the name - a bare name, or overloads of one qualified
    /// name - the most-called candidate is picked, and the answer counts the others in passed_over_from and lists the
    /// most called of them as <candidate end="from"/> records; pass a candidate's file as from_path_prefix and its line
    /// as from_line to pick it. A blank name is a tool error.
    pub(crate) from: String,
    /// Repo-relative path prefix with forward slashes: from is picked only among the symbols in files whose path
    /// starts with it. Omitted picks from every file.
    #[serde(default)]
    pub(crate) from_path_prefix: Option<String>,
    /// 1-based line from starts on: from is picked only among the symbols that start on it, which reaches one of
    /// several symbols of the name in one file. Omitted or null picks from every line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) from_line: Option<NonZeroU32>,
    /// The symbol the chain ends at: the last callee, named, read and picked the same way as from, its reading named
    /// in to_read_as= and the others that share its name counted in passed_over_to and listed as <candidate end="to"/>
    /// records, which to_path_prefix and to_line pick. A blank name is a tool error.
    pub(crate) to: String,
    /// Repo-relative path prefix with forward slashes: to is picked only among the symbols in files whose path starts
    /// with it. Omitted picks from every file.
    #[serde(default)]
    pub(crate) to_path_prefix: Option<String>,
    /// 1-based line to starts on: to is picked only among the symbols that start on it, which reaches one of several
    /// symbols of the name in one file. Omitted or null picks from every line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) to_line: Option<NonZeroU32>,
    /// Optional language filter (multiselect). Omit to search every indexed language - a pair resolving in several
    /// graphs yields one match per language. Lowercase language names - the set index_repo / index_stats report;
    /// anything else is a tool error listing the accepted set. A valid name absent from this repo contributes no
    /// match. An empty list selects no language, so the answer is empty. Null is equivalent to omission.
    /// A language of a family filters as the whole family, as typescript, tsx and javascript each search the jsts
    /// graph, and the answer's languages= names the families searched.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Longest chain searched, in calls, from 1 through 4294967295: 1 finds only a direct call. Omitted or null uses
    /// the default 8.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) hops: Option<NonZeroU32>,
    /// Drop call edges the resolver is less sure about than this floor, on every hop. "any" (the default) keeps every
    /// edge including name-only guesses. "likely" keeps edges backed
    /// by module structure and better; "certain" keeps only structurally resolved edges. Weak hops in answers are
    /// marked confidence="likely" / "guess".
    #[serde(default)]
    pub(crate) min_confidence: Option<ConfidenceFloor>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindReferencesParams {
    /// The symbol whose uses to list. A qualified name from a find_symbols hit (e.g. "pkg.Config") picks the symbol of
    /// that name; a name qualified by its owner with its language's separator ("Service.Greet" in Go, Python or
    /// TypeScript, "Service::greet" in Rust or C++) picks the symbol it names; a bare name picks the unique match. A
    /// name that selects nothing as written is read with each "." as "::", or each "::" as ".", and read_as= names
    /// that reading. Where several share the name - a bare name, or overloads of one qualified name - the candidate
    /// with the most recorded sites is picked, the answer counts the others in passed_over and lists the most used of
    /// them as <candidate/> records; pass a candidate's file as path_prefix and its line as line to pick it. A blank
    /// name is a tool error.
    pub(crate) name: String,
    /// Repo-relative path prefix with forward slashes: only the symbols in files whose path starts with it are picked
    /// from, e.g. a <candidate/>'s file, or "vendor/" for the vendored copy of a package. Omitted picks from every
    /// file.
    #[serde(default)]
    pub(crate) path_prefix: Option<String>,
    /// 1-based line the symbol starts on, e.g. a <candidate/>'s loc line: only the symbols that start on it are
    /// picked from. With path_prefix set to the loc's file, it is how to reach one of several symbols of the name in
    /// one file - C++ overloads, a declaration beside its definition. Omitted or null picks from every line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) line: Option<NonZeroU32>,
    /// Optional language filter (multiselect). Omit to search every indexed language - a name resolving in several
    /// graphs yields one match per language. Lowercase language names - the set index_repo / index_stats report;
    /// anything else is a tool error listing the accepted set. A valid name absent from this repo contributes no
    /// match. An empty list selects no language, so the answer is empty. Null is equivalent to omission.
    /// A language of a family filters as the whole family, as typescript, tsx and javascript each search the jsts
    /// graph, and the answer's languages= names the families searched.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Drop sites the resolver is less sure about than this floor: "any" (the default) keeps every site, "likely"
    /// keeps sites backed by module structure and better, "certain" keeps only structurally resolved sites.
    /// below_confidence= on a match counts the sites and the file_scope files the floor dropped.
    #[serde(default)]
    pub(crate) min_confidence: Option<ConfidenceFloor>,
    /// Keep only the uses that read the symbol ("read"), that set it ("write") or that name it as a type ("type"),
    /// e.g. "write" for "who sets Config.Timeout?"; an update (+=, ++) is a read and a write, a use as a type is also
    /// a read, and calls are dropped. Omit to keep every site. The counts on the match still count every site.
    #[serde(default)]
    pub(crate) access: Option<AccessFilter>,
}

/// Which uses a `find_references` answer keeps.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AccessFilter {
    /// The uses that read the symbol: reads, updates and uses as a type.
    Read,
    /// The uses that set the symbol: writes and updates.
    Write,
    /// The uses that name the symbol as a type.
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
    /// Symbol names to read, 1 to 10, each bare ("New") or qualified by its package or owner with its language's
    /// separator ("pkg.New", "Service.Greet" in Go, Python or TypeScript, "Service::greet" in Rust or C++). A qualified
    /// name selects every definition whose qualified name equals it or ends with it at a separator, and every
    /// definition named by its last segment whose own qualified name the name ends with, so "ns::Service::Greet" also
    /// selects "Service::Greet". A name that selects nothing as written is read with each "." as "::", or each "::" as
    /// ".", and read_as= on its <name> names that reading. An empty list or a blank name is a tool error.
    #[schemars(length(min = 1, max = 10), inner(length(min = 1)))]
    pub(crate) names: Vec<String>,
    /// Repository-relative path of the file the names are read for, as returned by find_files. Definitions in its
    /// directory rank first. Omitted, every definition ranks as other or vendored.
    #[serde(default)]
    pub(crate) from: Option<String>,
    /// Optional language filter (multiselect). Omit to read from every indexed language. Lowercase language names -
    /// the set index_repo / index_stats report; anything else is a tool error listing the accepted set. A valid name
    /// absent from this repo contributes no definitions. An empty list selects no language, so every name answers no
    /// definition. Null is equivalent to omission.
    /// A language of a family filters as the whole family, as typescript, tsx and javascript each search the jsts
    /// graph, and the answer's languages= names the families searched.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
    /// Positive maximum definitions per name. Omitted or null uses 3. Zero and negatives are rejected.
    #[serde(default)]
    pub(crate) limit: Option<NonZeroUsize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindImplementationsParams {
    /// The interface / base class / trait / mixed-in module / type alias name, e.g. "Provider". Bare or qualified with
    /// its language's separator ("io.Reader" in Go, "ui::Widget" in Rust or C++); a name that selects nothing as
    /// written is read with each "." as "::", or each "::" as ".", and read_as= names that reading. Take the exact name
    /// from a find_symbols hit when unsure. Where several types share the name, the most implemented is picked: the
    /// answer names it in <root/>, counts the others in passed_over and lists the most implemented of them as
    /// <candidate/> records, which path_prefix with a candidate's file and line with its line pick. A type alias
    /// answers the types whose parent names the alias, not those whose parent names its target. A blank name is a
    /// tool error.
    pub(crate) name: String,
    /// Repo-relative path prefix with forward slashes: only the types in files whose path starts with it are picked
    /// from, e.g. a <candidate/>'s file. Omitted picks from every file.
    #[serde(default)]
    pub(crate) path_prefix: Option<String>,
    /// 1-based line the type starts on, e.g. a <candidate/>'s loc line: only the types that start on it are picked
    /// from. With path_prefix set to the loc's file, it is how to reach one of several types of the name in one file -
    /// a template beside its specialization. Omitted or null picks from every line.
    #[serde(default)]
    #[schemars(range(max = u32::MAX))]
    pub(crate) line: Option<NonZeroU32>,
    /// Optional language filter (multiselect). Omit to query every indexed language; narrow when you know the
    /// contract's language, e.g. ["go"]. Lowercase language names - the set index_repo / index_stats report; anything
    /// else is a tool error listing the accepted set. A valid name absent from this repo contributes no results. An
    /// empty list selects no language, so the answer is empty. Null is equivalent to omission.
    /// A language of a family filters as the whole family, as typescript, tsx and javascript each search the jsts
    /// graph, and the answer's languages= names the families searched.
    #[serde(default)]
    pub(crate) languages: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileParams {
    /// Repo-relative path with forward slashes, exactly as returned by find_files or find_symbols (e.g.
    /// "pkg/service.go") - not absolute, not a bare filename.
    pub(crate) file: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImpactOfEditParams {
    /// Repo-relative path with forward slashes, exactly as returned by find_files or find_symbols (e.g.
    /// "pkg/service.go") - not absolute, not a bare filename.
    pub(crate) file: String,
    /// First line of the span, 1-based inclusive - e.g. the first changed line of a diff hunk.
    pub(crate) start_line: NonZeroU32,
    /// Last line of the span, 1-based inclusive. Omitted or null = the end of the file; a value before start_line is a
    /// tool error. A symbol counts as in-span when its definition overlaps the range at all.
    #[serde(default)]
    pub(crate) end_line: Option<NonZeroU32>,
}
