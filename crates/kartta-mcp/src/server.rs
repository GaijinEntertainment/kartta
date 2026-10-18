use std::fmt;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::sync::Arc;

use kartta::{Graph, Hit, Limits, Region, Search, SourceOptions};
use kartta_fs::{
    Case, Filter, FuzzyOptions, GlobOptions, GrepOptions, ListError, ListOptions, Patterns,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

use crate::config::Config;
use crate::diff::{self, Changed};
use crate::languages::{Catalog, echo};
use crate::params::{
    CallerRole, FileParams, FindCallPathParams, FindCalleesParams, FindCallersParams,
    FindFilesParams, FindImplementationsParams, FindReferencesParams, FindSymbolsParams,
    FindUnusedParams, GlobFilesParams, GrepParams, ImpactOfChangeParams, ImpactOfEditParams,
    IndexRepoParams, IndexStatsParams, ListDirParams, ReadDefinitionsParams,
};
use crate::render::{
    self, CallersAsk, Found, Globbed, NameDefinitions, PathAsk, Read, SUGGESTIONS, Suggestion,
    UnusedAsk,
};
use crate::state::{GraphRow, Indexed, Request, Slot, Status};

/// How many matching lines `grep` answers when the call names no cap.
const GREP_MATCHES: usize = 100;

/// How many bytes of a line `grep` answers: minified code puts a whole file on one line.
const GREP_LINE_BYTES: usize = 1024;

/// The largest file `grep` reads.
const GREP_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// What an unscoped `grep` leaves out when the call names no excludes: vendored and generated trees.
const GREP_EXCLUDES: [&str; 4] = ["**/vendor", "**/node_modules", "**/dist", "**/__pycache__"];

/// How deep `list_dir` lists when the call names no depth.
const LIST_DEPTH: usize = 1;

/// How many entries `list_dir` lists when the call names no cap.
const LIST_ENTRIES: usize = 1000;

/// The directories `list_dir` does not enter when the call names none: vendored and generated trees.
const LIST_COLLAPSED: [&str; 5] = ["vendor", "node_modules", "dist", "build", "__pycache__"];

/// How many hits `find_symbols` answers when the call names no limit.
const SYMBOLS: usize = 20;

/// How many names one `read_definitions` call reads at most.
const DEFINITION_NAMES: usize = 10;

/// How many definitions `read_definitions` answers per name when the call names no limit.
const DEFINITIONS: usize = 3;

/// How many lines of one definition `read_definitions` answers.
const SOURCE_LINES: usize = 500;

/// How many bytes of a line `read_definitions` answers.
const SOURCE_LINE_BYTES: usize = 1024;

/// How many implementors `find_implementations` lists.
const IMPLEMENTORS: usize = 500;

/// How many includes away `find_importers` follows the files that include a file.
const IMPORTER_DEPTH: u32 = 3;

/// How many calls long a chain `find_call_path` looks for when the call names no bound.
const PATH_HOPS: u32 = 8;

/// How many symbols a call path search reaches before it stops.
const PATH_MAX_SYMBOLS: usize = 20_000;

/// How many sites `find_references` lists.
const REFERENCES: usize = 500;

/// How many calls away `impact_of_edit` walks the callers a change reaches, and the files that include the file.
const IMPACT_DEPTH: u32 = 3;

/// How many symbols the walk over a changed symbol's callers reaches before it stops.
const IMPACT_MAX_SYMBOLS: usize = 500;

/// How many changed symbols, and how many hotspots, `impact_of_change` lists when the call names no limit.
const CHANGE_LISTED: usize = 20;

/// How many unused definitions `find_unused` lists when the call names no limit.
const UNUSED: usize = 50;

/// How many hops `find_callers` walks when the call names none.
const CALLER_HOPS: u32 = 3;

/// How many symbols a walk over callers or callees reaches before it stops, the symbol it starts from included.
const CALLERS_MAX_SYMBOLS: usize = 500;

/// How many hops `find_callees` walks when the call names none: a body's callees fan out at every hop.
const CALLEE_HOPS: u32 = 1;

/// The MCP server: the tools, over the one repository it indexes.
#[derive(Clone)]
pub struct Server {
    slot: Arc<Slot>,
    config: Arc<Config>,
    catalog: Arc<Catalog>,
    tool_router: ToolRouter<Self>,
}

impl fmt::Debug for Server {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Server")
            .field("slot", &self.slot)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// A server whose builds take `config`, with nothing indexed.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self {
            slot: Arc::default(),
            config: Arc::new(config),
            catalog: Arc::new(Catalog::new()),
            tool_router: Self::tool_router(),
        }
    }

    /// Starts building the index of `root` with every language, as `index_repo` with no other argument does, and
    /// returns without waiting for it. Until the build finishes, `index_stats` answers that it is building and every
    /// other query answers an error saying so; an `index_repo` of the same root waits for it.
    ///
    /// The build runs on the Tokio runtime's blocking threads, so this is called from within that runtime.
    pub fn index_at_launch(&self, root: PathBuf) {
        let request = Request {
            root,
            languages: self.catalog.to_build(None),
            index_ignored: false,
        };
        drop(self.slot.start(request, self.config.build_options(false)));
    }

    /// Answers a query from the current index on a blocking thread, and on the index's own threads.
    async fn answer<F>(&self, answer: F) -> Result<String, String>
    where
        F: FnOnce(&Indexed) -> Result<String, String> + Send + 'static,
    {
        let indexed = self.slot.current()?;
        tokio::task::spawn_blocking(move || indexed.engine.index().on_threads(|| answer(&indexed)))
            .await
            .map_err(|error| format!("the query stopped before it answered: {error}"))?
    }
}

/// A definition a name selects in one family's graph, and how near it sits.
type Nearest<'g> = (&'g str, kartta::Found<'g>);

/// The family graphs a query filtered by `languages` searches, in the engine's order.
fn selected<'g>(
    indexed: &'g Indexed,
    catalog: &Catalog,
    languages: Option<&[String]>,
) -> Vec<(&'g str, &'g Graph)> {
    indexed
        .engine
        .graphs()
        .filter(|(family, _)| catalog.selects(languages, family))
        .collect()
}

/// Compiles a list of globs, or `None` for an empty one.
fn globs(patterns: &[String]) -> Result<Option<Filter>, String> {
    if patterns.is_empty() {
        return Ok(None);
    }
    Filter::new(patterns)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Up to [`SUGGESTIONS`] close alternatives to `name` from the graphs a query searched, best first, none of them
/// named `name` itself.
fn suggestions<'g>(indexed: &'g Indexed, families: &[&'g str], name: &str) -> Vec<Suggestion<'g>> {
    let mut found: Vec<Suggestion<'g>> = indexed
        .engine
        .graphs()
        .filter(|(family, _)| families.contains(family))
        .flat_map(|(family, graph)| {
            graph
                .search(name, SUGGESTIONS)
                .hits
                .into_iter()
                .map(move |hit| Suggestion { family, hit })
        })
        .filter(|suggestion| {
            suggestion.hit.definition.name != name
                && suggestion.hit.definition.qualified_name != name
        })
        .collect();
    found.sort_by(|left, right| right.hit.score.total_cmp(&left.hit.score));
    found.truncate(SUGGESTIONS);
    found
}

#[tool_router]
impl Server {
    #[tool(
        description = "Build the code index for a repository: walk the file tree and parse every recognized source \
                       file into a per-language code graph of symbols (functions, types, methods...) with their \
                       calls, uses and includes. Run this first - every other tool queries this index and errors \
                       until it exists. Takes seconds on large repositories; one repository is indexed per server, \
                       and indexing another replaces the previous one (a build of the same root and languages already \
                       under way is waited for, not repeated). Returns an <index> envelope with one <language/> record \
                       per built graph (files, symbols, calls, uses and includes, plus 'files_without_symbols' when a \
                       language's files did not all yield symbols and 'unheld' when some files it claims are binary or \
                       unreadable), and an <errors> block listing what the walk could not read - those language names \
                       are the valid values for the 'languages' filter of the symbol tools."
    )]
    async fn index_repo(
        &self,
        Parameters(p): Parameters<IndexRepoParams>,
    ) -> Result<String, String> {
        self.catalog.check(p.languages.as_deref())?;
        let request = Request {
            root: PathBuf::from(&p.root),
            languages: self.catalog.to_build(p.languages.as_deref()),
            index_ignored: p.index_ignored,
        };
        let mut outcome = self
            .slot
            .start(request, self.config.build_options(p.index_ignored));
        let built = outcome
            .wait_for(Option::is_some)
            .await
            .map_err(|_| "the build stopped before it answered".to_owned())?
            .clone();
        let indexed = built.ok_or_else(|| "the build answered nothing".to_owned())??;
        Ok(render::index(&indexed, &[]))
    }

    #[tool(
        description = "Show what the index currently holds: repository root, total files, and per-language \
                       file/symbol/call/use/include counts, plus 'files_without_symbols' on any language whose files \
                       did not all yield symbols (an unsupported dialect or a failed parse) and 'unheld' on any whose \
                       claimed files are binary or unreadable - each absent when it is zero. Use it to recall which \
                       languages are present (the valid 'languages' values for the symbol tools), to gauge how much \
                       code each language has, to tell a genuinely empty answer from missing coverage, or to check \
                       whether a repository is indexed at all. While a build runs it answers status=\"building\" \
                       with the seconds so far. Returns the same <index> envelope index_repo produced. With \
                       coverage=true it also resolves every call site and adds one <coverage/> record per language: \
                       sites, bound (split into certain / likely / guess, the confidence marks of the call tools), \
                       edges (distinct caller-callee pairs), and why the other sites bound no definition, each only \
                       when it is not zero - external_import (through an import of a module outside the repository), \
                       not_exported, undefined (a builtin or code outside the graph), not_admitted, ambiguous (several \
                       candidates, none nearer), and through_receiver (of the undefined and not admitted, calls on a \
                       value of a type the graph does not define). Use it to tell \"no callers\" from a language whose \
                       calls the graph binds poorly. coverage resolves every call in the repository, about half a \
                       second on a large one, each time it is asked."
    )]
    async fn index_stats(
        &self,
        Parameters(p): Parameters<IndexStatsParams>,
    ) -> Result<String, String> {
        match self.slot.status() {
            Status::Building { root, elapsed } => Ok(render::building(&root, elapsed)),
            Status::Ready(indexed) if !p.coverage => Ok(render::index(&indexed, &[])),
            Status::Ready(_) => {
                self.answer(|indexed| Ok(render::index(indexed, &indexed.engine.resolution(0))))
                    .await
            }
            Status::Empty | Status::Failed { .. } => self
                .slot
                .current()
                .map(|indexed| render::index(&indexed, &[])),
        }
    }

    #[tool(
        description = "Find files by name with a fuzzy match: pattern characters must appear in order but not \
                       contiguously, and filename matches rank above directory matches (\"serv\" finds \
                       pkg/service.go). Use it to locate a file when you know part of its name or path: a config \
                       file, a module, a handler. Covers ALL indexed files (configs, docs, assets), not just source \
                       code; optional include/exclude globs narrow the candidates first. To enumerate files by \
                       pattern alone - every .ts file, everything under src/ - use glob_files instead. Returns \
                       matching repo-relative paths, one per line, best first - exactly the path format the 'file' \
                       parameters of other tools expect; a path holding a control character such as a line break, or \
                       opening with a quote, is written quoted with backslash escapes (\\n, \\t, \\\", \\\\). \
                       Requires index_repo first."
    )]
    async fn find_files(
        &self,
        Parameters(p): Parameters<FindFilesParams>,
    ) -> Result<String, String> {
        if p.pattern.split_whitespace().next().is_none() {
            return Err(
                "pattern is required - to list files by glob alone, use glob_files".to_owned(),
            );
        }
        let includes = globs(&p.includes)?;
        let excludes = globs(&p.excludes)?;
        self.answer(move |indexed| {
            let index = indexed.engine.index();
            let mut options = FuzzyOptions::default();
            if let Some(includes) = &includes {
                options = options.filter(includes);
            }
            if excludes.is_none() {
                options = options.limit(p.max_results);
            }
            let ranked = index.fuzzy(&p.pattern, &options);
            let mut paths = index.paths();
            let wanted = if p.max_results == 0 {
                usize::MAX
            } else {
                p.max_results
            };
            let found: Vec<String> = ranked
                .iter()
                .map(|found| paths.get(found.position).to_owned())
                .filter(|path| {
                    excludes
                        .as_ref()
                        .is_none_or(|excludes| !excludes.matches(path))
                })
                .take(wanted)
                .collect();
            Ok(render::paths(&p.pattern, &found))
        })
        .await
    }

    #[tool(
        description = "List indexed files matching glob patterns - pure enumeration: every .go file, everything \
                       under src/, the tree minus vendored code. Use it when the SET of files is the answer \
                       (inventory, scoping a sweep); when you are hunting ONE file by name, use find_files. Returns \
                       matching repo-relative paths, one per line, in sorted path order, quoted as find_files quotes \
                       them; a list max_results cut ends with a (truncated: ...) line saying how many matched when \
                       the search counted them, and an (excludes removed: N) line counts the matches the excludes \
                       dropped. Requires index_repo first."
    )]
    async fn glob_files(
        &self,
        Parameters(p): Parameters<GlobFilesParams>,
    ) -> Result<String, String> {
        if p.globs.is_empty() {
            return Err(
                "globs is required and must be non-empty - pass [\"**\"] to list the whole tree"
                    .to_owned(),
            );
        }
        let filter = Filter::new(&p.globs).map_err(|error| error.to_string())?;
        let excludes = globs(&p.excludes)?;
        self.answer(move |indexed| {
            let index = indexed.engine.index();
            let limit = if excludes.is_some() { 0 } else { p.max_results };
            let found = index.glob(&filter, &GlobOptions::default().limit(limit));
            let mut paths = index.paths();
            let mut listed = crate::llmxml::List::default();
            let mut matched = 0;
            for &position in found.items() {
                let path = paths.get(position);
                if excludes
                    .as_ref()
                    .is_some_and(|excludes| excludes.matches(path))
                {
                    continue;
                }
                matched += 1;
                if p.max_results == 0 || listed.len() < p.max_results {
                    listed.push(path);
                }
            }
            let globbed = if found.truncated() {
                Globbed::Cut { matched: None }
            } else if matched > listed.len() {
                Globbed::Cut {
                    matched: Some(matched),
                }
            } else {
                Globbed::Whole
            };
            let excluded = found.items().len() - matched;
            Ok(render::globbed(
                &p.globs.join(", "),
                listed,
                globbed,
                excluded,
            ))
        })
        .await
    }

    #[tool(
        description = "Search file contents line by line with regular expressions - the evidence tool: pull the \
                       exact lines that prove how code behaves, find every textual occurrence of an identifier, \
                       string, or pattern. Several patterns search as one pass (a line matching any of them is \
                       returned). Covers the indexed files; unscoped searches use the default exclusions documented \
                       by 'excludes'. A search scoped with non-empty 'includes' and omitted 'excludes' covers exactly \
                       what it includes. Returns a <grep> envelope: one <file path=..> block per matched file with \
                       'NUMBER:text' match lines (and 'NUMBER-text' context when context_lines is set), capped at \
                       max_results match lines (default 100) spent in file-path order. A line longer than 1024 \
                       bytes is cut around its match and marked \
                       [... omitted N bytes]. Binary files and files over 4 MiB are skipped and counted in binary= \
                       and oversized=; unreadable files are listed in <failures>; unsearched= counts the files the \
                       search did not reach once the cap filled, excluded= the files the excludes dropped, \
                       ruled_out= the files the content index, built from their content as index_repo read it, \
                       shows hold no match, and symlinks= the links \
                       selected, which are never read through. The \
                       answer reads each file as it is now, but the set of files is the checkout as index_repo saw \
                       it: a file added since is missing from it until index_repo runs again. For symbol definitions \
                       use find_symbols, for call sites find_callers - grep finds text, not structure. Requires \
                       index_repo first."
    )]
    async fn grep(&self, Parameters(p): Parameters<GrepParams>) -> Result<String, String> {
        if p.patterns.iter().all(|pattern| pattern.trim().is_empty()) {
            return Err("patterns needs at least one non-blank pattern".to_owned());
        }
        let case = if p.case_insensitive.unwrap_or(false) {
            Case::Insensitive
        } else {
            Case::Sensitive
        };
        let patterns = Patterns::new(&p.patterns, case).map_err(|error| error.to_string())?;
        let includes = globs(&p.includes)?;
        let excludes = match &p.excludes {
            Some(excludes) => globs(excludes)?,
            None if p.includes.is_empty() => {
                Some(Filter::new(GREP_EXCLUDES).map_err(|error| error.to_string())?)
            }
            None => None,
        };
        self.answer(move |indexed| {
            let index = indexed.engine.index();
            let mut options = GrepOptions::default()
                .context(p.context_lines)
                .limit(p.max_results.map_or(GREP_MATCHES, NonZeroUsize::get))
                .max_line_bytes(GREP_LINE_BYTES)
                .max_file_bytes(GREP_FILE_BYTES);
            if let Some(includes) = &includes {
                options = options.filter(includes);
            }
            if let Some(excludes) = &excludes {
                options = options.exclude(excludes);
            }
            let matches = index.grep(&patterns, &options);
            Ok(render::grep(&p.patterns, &matches, &mut index.paths()))
        })
        .await
    }

    #[tool(
        description = "List a directory's entries from the index: subdirectories, files with their byte sizes \
                       (size=\"unknown\" when the file could not be read - distinct from size=\"0\", an empty file; \
                       directories and symlinks carry no size), and symlinks (marked explicitly, with their target). \
                       Use it when the SET of entries under one directory is the answer - what a package contains, \
                       whether a test or companion file exists beside a module. find_files hunts ONE file by name; \
                       glob_files enumerates by pattern across the whole tree. path \".\" (or omitted) lists the \
                       repository root; max_depth 1 (the default) shows immediate children only. Descent into \
                       vendored and generated trees (vendor, node_modules, dist, build, __pycache__) is suppressed by \
                       default - the directory entry itself still appears, and listing it by path shows its \
                       contents. Returns a <dir> envelope with one <entry/> record per entry in name-sorted order, a \
                       directory immediately preceding its children; a directory the listing did not enter (below \
                       max_depth or suppressed) carries unlisted= with the count of entries beneath it. Capped at \
                       max_entries (default 1000), truncated=\"true\" when the cap cut it; total= on <dir> counts \
                       every entry the index holds under the path when the listing shows fewer, and excluded= the \
                       entries skip_dotfiles dropped. An empty directory is a successful empty answer, not an error. \
                       The entries are the indexed universe - what the walk saw, not the live filesystem - while a \
                       file's size is read from the filesystem when the file is first listed. Requires index_repo \
                       first."
    )]
    async fn list_dir(&self, Parameters(p): Parameters<ListDirParams>) -> Result<String, String> {
        let names = p
            .excludes
            .unwrap_or_else(|| LIST_COLLAPSED.map(str::to_owned).to_vec());
        let collapse = globs(
            &names
                .iter()
                .map(|name| format!("**/{}", escape_glob(name)))
                .collect::<Vec<_>>(),
        )?;
        let dotfiles = if p.skip_dotfiles.unwrap_or(false) {
            Some(Filter::new(["**/.*"]).map_err(|error| error.to_string())?)
        } else {
            None
        };
        if p.path.starts_with('/') {
            return Err(format!(
                "{:?} is absolute - pass a repo-relative directory path as printed by find_files; \".\" lists the root",
                p.path
            ));
        }
        let dir = match p.path.trim_end_matches('/') {
            "." => String::new(),
            path => path.to_owned(),
        };
        self.answer(move |indexed| {
            let mut options = ListOptions::default()
                .depth(p.max_depth.map_or(LIST_DEPTH, NonZeroUsize::get))
                .limit(p.max_entries.map_or(LIST_ENTRIES, NonZeroUsize::get));
            if let Some(collapse) = &collapse {
                options = options.collapse(collapse);
            }
            if let Some(dotfiles) = &dotfiles {
                options = options.exclude(dotfiles);
            }
            let listing = indexed
                .engine
                .index()
                .list(&dir, &options)
                .map_err(|error| match error {
                    ListError::NotADirectory { .. } => format!("{error} - list its parent directory instead"),
                    error => format!(
                        "{error} - pass a repo-relative directory path as printed by find_files; \".\" lists the root"
                    ),
                })?;
            Ok(render::dir(&dir, &listing))
        })
        .await
    }

    #[tool(
        description = "Find where symbols - functions, methods, types, constants, variables - are defined, by keyword \
                       search over the words of symbol names and of their files' paths. The starting point for \
                       \"where is X defined?\" and for pinning a symbol's exact name before find_callers / \
                       find_implementations. Searches every indexed language unless narrowed with 'languages'. \
                       Returns a <symbols> envelope, one ranked <hit/> record per match: name, optional qn (the \
                       qualified name, when it says more), optional owner (the declaring type or namespace), kind, \
                       lang, loc (file:start-end, the definition's full span, or file:line for one line), and a \
                       one-line sig. A hit whose name is the query outranks every other. To read a definition's \
                       source, pass its name to read_definitions. When keyword matches underfill the limit, fuzzy \
                       name matches top the answer up - those records carry fuzzy=true and are not matches for the \
                       query as requested. truncated=true with total= on <symbols> means the limit cut the answer: a \
                       higher limit or a narrower query shows more; unmatched= names the query's words no definition \
                       holds. A record carrying a 'role' attribute is a reach artifact rather than ordinary code - one \
                       of test, entry or generated, in that precedence; an ordinary record carries none. Symbols in \
                       GENERATED or vendored code rank below hand-written ones. An answer with NO hits says so, and \
                       names any searched language holding files that yielded no symbols (an unsupported dialect or a \
                       failed parse) - read that as missing coverage rather than as the symbol not existing, and call \
                       index_stats for the per-language counts. Requires index_repo first."
    )]
    async fn find_symbols(
        &self,
        Parameters(p): Parameters<FindSymbolsParams>,
    ) -> Result<String, String> {
        self.catalog.check(p.languages.as_deref())?;
        let catalog = Arc::clone(&self.catalog);
        self.answer(move |indexed| {
            let limit = p.limit.map_or(SYMBOLS, NonZeroUsize::get);
            let searched = selected(indexed, &catalog, p.languages.as_deref());
            let searches: Vec<(&str, Search<'_>)> = searched
                .iter()
                .map(|&(family, graph)| (family, graph.search(&p.query, limit)))
                .collect();
            let keyword: usize = searches.iter().map(|(_, search)| search.total).sum();
            let fill_up = keyword < limit;
            let matched = if fill_up {
                keyword
                    + searches
                        .iter()
                        .map(|(_, search)| search.fuzzy_total.unwrap_or(0))
                        .sum::<usize>()
            } else {
                keyword
            };
            let dropped_words = searches
                .iter()
                .map(|(_, search)| search.dropped_words)
                .max()
                .unwrap_or(0);
            let unmatched = searches
                .split_first()
                .map_or_else(Vec::new, |((_, first), rest)| {
                    first
                        .unmatched_words
                        .iter()
                        .filter(|word| {
                            rest.iter()
                                .all(|(_, search)| search.unmatched_words.contains(word))
                        })
                        .cloned()
                        .collect()
                });
            let mut hits: Vec<(&str, Hit<'_>)> = searches
                .into_iter()
                .flat_map(|(family, search)| search.hits.into_iter().map(move |hit| (family, hit)))
                .filter(|(_, hit)| fill_up || !hit.fuzzy)
                .collect();
            hits.sort_by(|left, right| {
                left.1
                    .fuzzy
                    .cmp(&right.1.fuzzy)
                    .then_with(|| right.1.score.total_cmp(&left.1.score))
            });
            hits.truncate(limit);
            let coverage: Vec<&GraphRow> = if hits.is_empty() {
                indexed
                    .graphs
                    .iter()
                    .filter(|row| row.files_without_symbols > 0)
                    .filter(|row| searched.iter().any(|(family, _)| *family == row.family))
                    .collect()
            } else {
                Vec::new()
            };
            let found = Found {
                hits,
                matched,
                unmatched,
                dropped_words,
            };
            Ok(render::hits(
                &p.query,
                &echo(p.languages.as_deref()),
                &found,
                &coverage,
            ))
        })
        .await
    }

    #[tool(
        description = "Read the source of named definitions - functions, methods, types - several names in one \
                       call. Answers \"show me X\" without a search followed by a file read of a guessed range. \
                       Searches every indexed language unless narrowed with 'languages'. Returns a <definitions> \
                       envelope with one <name of=.. total=..> per requested name, in request order; each holds its \
                       <definition> records nearest first, carrying name, optional qn, optional owner, kind, lang, loc \
                       (file:start-end, the full span, or file:line for one line), scope, and a role for test, entry, \
                       or generated code. scope is same-directory (beside 'from'), vendored, or other; it ranks by \
                       location and does not resolve which definition a given call binds to. The body is the file \
                       text from the doc block directly above the definition through its last line, each line \
                       numbered, at most 500 lines with each line cut at 1024 bytes. loc is the span at index_repo \
                       time and the body is read at call time, so a file edited since answers its new text at the \
                       old span, and resized=true marks a file whose size changed since. truncated=true marks a body \
                       cut before the span ends, or missing because the file no longer reaches the span; a definition \
                       whose file could not be read has no body, carries error= with the reason and its sig when one \
                       was captured. total on a <name> counts every definition the name selects; shown=N means the \
                       limit cut the list to N. A name nothing defines answers total=0 with a find_symbols hint. \
                       Requires index_repo first."
    )]
    async fn read_definitions(
        &self,
        Parameters(p): Parameters<ReadDefinitionsParams>,
    ) -> Result<String, String> {
        if p.names.is_empty() {
            return Err("names is empty; pass at least one definition name".to_owned());
        }
        if p.names.len() > DEFINITION_NAMES {
            return Err(format!(
                "names must list at most {DEFINITION_NAMES} names (got {}); split the list",
                p.names.len()
            ));
        }
        if p.names.iter().any(String::is_empty) {
            return Err("names must not contain an empty name".to_owned());
        }
        self.catalog.check(p.languages.as_deref())?;
        let catalog = Arc::clone(&self.catalog);
        self.answer(move |indexed| {
            let limit = p.limit.map_or(DEFINITIONS, NonZeroUsize::get);
            let searched = selected(indexed, &catalog, p.languages.as_deref());
            let per_name: Vec<(usize, Vec<Nearest<'_>>)> = p
                .names
                .iter()
                .map(|name| {
                    let mut total = 0;
                    let mut found = Vec::new();
                    for &(family, graph) in &searched {
                        let definitions = graph.definitions(name, p.from.as_deref(), limit);
                        total += definitions.total;
                        found.extend(definitions.found.into_iter().map(|found| (family, found)));
                    }
                    found.sort_by_key(|(_, found)| found.nearness);
                    found.truncate(limit);
                    (total, found)
                })
                .collect();
            let wanted: Vec<_> = per_name
                .iter()
                .flat_map(|(_, found)| found.iter().map(|(_, found)| found.definition))
                .collect();
            let options = SourceOptions::default()
                .max_lines(SOURCE_LINES)
                .max_line_bytes(SOURCE_LINE_BYTES);
            let mut sources = indexed.engine.sources(&wanted, options).into_iter();
            let answers: Vec<NameDefinitions<'_>> = p
                .names
                .iter()
                .zip(per_name)
                .map(|(name, (total, found))| NameDefinitions {
                    name,
                    total,
                    definitions: found
                        .into_iter()
                        .zip(&mut sources)
                        .map(|((family, found), source)| Read {
                            family,
                            definition: found.definition,
                            nearness: found.nearness,
                            source,
                        })
                        .collect(),
                })
                .collect();
            Ok(render::definitions(&echo(p.languages.as_deref()), &answers))
        })
        .await
    }

    #[tool(
        description = "List the types that implement or extend a named interface, base class, or trait, \
                       transitively. Use it when a contract changes and you need everything bound to it, or to find \
                       the real targets behind a dynamically-dispatched call site. Searches every indexed language \
                       unless narrowed with 'languages'. Returns an <implementations> envelope, one <type/> record \
                       per implementor with its lang and loc (file:line), nearest first; depth=N marks one that \
                       implements the type through N-1 others. At most 500 are listed, and total= then counts them \
                       all; passed_over= counts the other types the name selects. A type that implements an \
                       interface without naming it, as Go's structural typing allows, is not found. An unknown name \
                       answers with up to 5 close alternatives (did you mean). Requires index_repo first."
    )]
    async fn find_implementations(
        &self,
        Parameters(p): Parameters<FindImplementationsParams>,
    ) -> Result<String, String> {
        self.catalog.check(p.languages.as_deref())?;
        let catalog = Arc::clone(&self.catalog);
        self.answer(move |indexed| {
            let searched = selected(indexed, &catalog, p.languages.as_deref());
            let mut found: Vec<_> = searched
                .iter()
                .filter_map(|&(family, graph)| {
                    Some((
                        family,
                        graph.implementations(&p.name, u32::MAX, IMPLEMENTORS)?,
                    ))
                })
                .collect();
            let mut room = IMPLEMENTORS;
            for (_, implementations) in &mut found {
                implementations.found.truncate(room);
                room -= implementations.found.len();
            }
            let families: Vec<&str> = searched.iter().map(|&(family, _)| family).collect();
            let suggestions = if found.is_empty() {
                suggestions(indexed, &families, &p.name)
            } else {
                Vec::new()
            };
            Ok(render::implementations(
                &p.name,
                &echo(p.languages.as_deref()),
                &found,
                &suggestions,
            ))
        })
        .await
    }

    #[tool(
        description = "Find every file that imports or includes a given file, directly or through a chain of imports \
                       (followed up to 3 hops) - the downstream files of a shared module, header, or type-definition \
                       file. This is whole-file impact: for line-level impact use impact_of_edit, for a single \
                       function use find_callers. Returns an <importers> envelope, one <file/> record per downstream \
                       file, nearest first; depth=N marks one that reaches the file through N-1 others, and \
                       unexpanded= counts the importers at the 3-hop limit whose own importers were not followed. A \
                       Go file counts every importer of its package. Requires index_repo first."
    )]
    async fn find_importers(
        &self,
        Parameters(p): Parameters<FileParams>,
    ) -> Result<String, String> {
        self.answer(move |indexed| {
            let dependents = indexed
                .engine
                .graph_of(&p.file)
                .and_then(|graph| graph.dependents(&p.file, IMPORTER_DEPTH, 0));
            Ok(render::importers(&p.file, dependents.as_ref()))
        })
        .await
    }

    #[tool(
        description = "List the definitions of one known file in source order - \"where in this file is X?\" when the \
                       file is known but the place in it is not. Returns an <outline> envelope with one <symbol/> \
                       record per definition: name; qn, the qualified name, omitted when it equals name; owner, the \
                       declaring type or namespace, omitted on a top-level symbol; kind; lines=\"start-end\" (the \
                       definition's full span, nested definitions inside their enclosing one's span); and sig, the \
                       declaration, omitted when none was captured. Read the one span you need instead of the whole \
                       file or a guessed window. A path no code graph holds and a held file with no recorded \
                       definition both report symbols=\"0\", told apart by the note. For callers and reach of a range \
                       use impact_of_edit. Requires index_repo first."
    )]
    async fn file_outline(&self, Parameters(p): Parameters<FileParams>) -> Result<String, String> {
        self.answer(move |indexed| {
            let outline = indexed
                .engine
                .graph_of(&p.file)
                .and_then(|graph| graph.outline(&p.file));
            Ok(render::outline(&p.file, outline.as_deref()))
        })
        .await
    }

    #[tool(
        description = "Find the shortest chain of calls from one symbol to another: from calls a symbol, which calls \
                       another, until one calls to. Answers \"does this entry point reach that function, and how?\" in \
                       one call, where following the calls by hand takes a read per hop. Searches every indexed \
                       language unless narrowed with 'languages'; a pair resolving in several languages yields one \
                       match per language. Returns a <call_path> envelope of <match lang=..> records: each the \
                       resolved from and to, the chain length in hops and the searched bound in max_hops, and one \
                       <ref> per caller in call order - the last one calls to - each carrying loc= at the caller's \
                       definition, call= at the line where it calls the next symbol, an explicit role= and a \
                       confidence=likely / guess mark on a weakly resolved hop. A match without a chain carries \
                       found=false and states that an absent chain in the graph does not show one absent in the code; \
                       at_depth_bound= counts the callers at the hops limit a longer chain could pass through, and \
                       below_confidence= the callers the floor dropped. A search the node cap (20000) stops carries \
                       truncated=true. When from and to resolve to one symbol, the match has hops=0 and no ref. \
                       Requires index_repo first."
    )]
    async fn find_call_path(
        &self,
        Parameters(p): Parameters<FindCallPathParams>,
    ) -> Result<String, String> {
        self.catalog.check(p.languages.as_deref())?;
        let catalog = Arc::clone(&self.catalog);
        self.answer(move |indexed| {
            let hops = p.hops.map_or(PATH_HOPS, NonZeroU32::get);
            let floor = p.min_confidence.floor();
            let limits = Limits::new(hops)
                .with_min_confidence(floor)
                .with_max_symbols(PATH_MAX_SYMBOLS);
            let paths: Vec<_> = selected(indexed, &catalog, p.languages.as_deref())
                .into_iter()
                .filter_map(|(family, graph)| {
                    Some((family, graph.call_path(&p.from, &p.to, limits)?))
                })
                .collect();
            let languages = echo(p.languages.as_deref());
            let ask = PathAsk {
                from: &p.from,
                to: &p.to,
                languages: &languages,
                hops,
                min_confidence: floor,
            };
            Ok(render::call_path(&ask, &paths))
        })
        .await
    }

    #[tool(
        description = "List every site where a symbol is used: its calls, and its uses as a type or as a \
                       package-qualified value. Answers \"where is this type, constant, or function used?\" in one \
                       call, without a text search that also matches strings, comments, and same-named symbols. \
                       Searches every indexed language unless narrowed with 'languages'; a name resolving in several \
                       languages yields one match per language. Returns a <references> envelope of <match lang=..> \
                       records: each the resolved symbol with its loc, kind, and site count, and one <ref> per site in \
                       file and line order, carrying the enclosing symbol's name, qn and loc, the enclosing symbol's \
                       explicit role= (\"consumer\", or test/entry/generated), site=call or site=use, at= for the \
                       site's file:line, and a confidence=likely / guess mark on a weakly resolved site. A caller is \
                       one site, at one of its calls; every other use is one site per line. Uses other than calls are \
                       recorded only for some languages, a method's calls through a value of its owner's type resolve \
                       only where the graph can name that type, and no access to a field is recorded, so a missing \
                       site does not show an unused symbol. A match lists at most 500 sites and then carries shown=; \
                       below_confidence= counts the sites the floor dropped and passed_over= the other symbols the \
                       name selects. An unknown name answers with up to 5 close alternatives (did you mean). Requires \
                       index_repo first."
    )]
    async fn find_references(
        &self,
        Parameters(p): Parameters<FindReferencesParams>,
    ) -> Result<String, String> {
        self.catalog.check(p.languages.as_deref())?;
        let catalog = Arc::clone(&self.catalog);
        self.answer(move |indexed| {
            let floor = p.min_confidence.floor();
            let searched = selected(indexed, &catalog, p.languages.as_deref());
            let found: Vec<_> = searched
                .iter()
                .filter_map(|&(family, graph)| {
                    Some((family, graph.references(&p.name, floor, REFERENCES)?))
                })
                .collect();
            let families: Vec<&str> = searched.iter().map(|&(family, _)| family).collect();
            let suggestions = if found.is_empty() {
                suggestions(indexed, &families, &p.name)
            } else {
                Vec::new()
            };
            let languages = echo(p.languages.as_deref());
            let ask = CallersAsk {
                name: &p.name,
                languages: &languages,
                role: None,
                floored: floor != 0,
            };
            Ok(render::references(&ask, &found, &suggestions))
        })
        .await
    }

    #[tool(
        description = "Assess the blast radius of editing a span of lines - \"what breaks if I touch these lines?\" \
                       for a diff hunk under review or an edit you are about to make. Returns an <impact> envelope \
                       with one <symbol> record per definition overlapping the range - caller and callee counts with \
                       sampled <ref/> lists (each carrying an explicit role=, \"consumer\" for a real caller/callee or \
                       test/entry/generated for a reach artifact, at= for the line of the call, and weakly resolved \
                       edges marked confidence=likely/guess; a shown=/of= pair on the enclosing <callers>/<callees> \
                       discloses the 50-ref sampling cap), unresolved= for the calls into code the graph does not \
                       hold, and the reach (how many symbols across how many files transitively depend on it within 3 \
                       hops, with a reach-band grade; truncated=true when the 500-symbol cap stopped the walk) - plus \
                       <includes> and <included-by> file lists. A range covering only file-scope code (imports, \
                       comments) reports zero symbols and still lists the file's includes and includers when it has \
                       any; zero symbols means nothing is defined there, not that the edit is safe. For one named \
                       function use find_callers; for whole-file impact use find_importers. Requires index_repo \
                       first."
    )]
    async fn impact_of_edit(
        &self,
        Parameters(p): Parameters<ImpactOfEditParams>,
    ) -> Result<String, String> {
        if p.start_line == 0 {
            return Err("start_line is 1-based - it must be >= 1".to_owned());
        }
        if p.end_line != 0 && p.end_line < p.start_line {
            return Err(format!(
                "end_line {} is before start_line {}",
                p.end_line, p.start_line
            ));
        }
        self.answer(move |indexed| {
            let end_line = if p.end_line == 0 {
                u32::MAX
            } else {
                p.end_line
            };
            let limits = Limits::new(IMPACT_DEPTH).with_max_symbols(IMPACT_MAX_SYMBOLS);
            let segment = indexed
                .engine
                .graph_of(&p.file)
                .and_then(|graph| graph.analyze_segment(&p.file, p.start_line..=end_line, limits));
            Ok(render::impact(
                &p.file,
                (p.start_line, end_line),
                segment.as_ref(),
            ))
        })
        .await
    }

    #[tool(
        description = "Assess what a whole change reaches - a diff of several hunks across files and languages in one \
                       call: the symbols it touches, how far a change to each spreads through its callers, and where \
                       those spreads meet. Use it to review a diff or plan a multi-file edit; for one span of one file \
                       with its callees and includes, use impact_of_edit. Pass the change as regions (file plus line \
                       range) or as the text of a unified diff, whose + side lines it takes (for a hunk that only \
                       deletes, the line on each side of the deletion). Returns a <change> \
                       envelope: changed_symbols and changed_files, the footprint (every symbol the changed symbols' \
                       callers reach within 3 hops, each once) and footprint_files; a <seeds> list of the changed \
                       symbols, widest reach first and test/entry/generated code last, each <seed> with its loc span, \
                       direct callers count, reach and reach_band, its direct callers as <ref/> records with at= the \
                       line of the call (at most 10, in <callers shown= of=>), or past 30 callers their directories \
                       as <dir path callers/> (in <caller_dirs shown= of=>); and, when two or more changed symbols \
                       reach one, a <hotspots> list of those symbols, reached_by= how many and hops= how near, \
                       changed=true on one the change touches itself. Each list shows at most 'limit' (default 20) \
                       with shown= of=. A change that overlaps no symbol answers changed_symbols=0 with a note in \
                       place of the lists. Each \
                       count of what was left out appears only when it is not zero: unheld_regions= the regions in \
                       files no code graph holds, regions_outside_symbols= those overlapping no symbol, \
                       truncated_walks= the changed symbols whose walk the 500-symbol cap stopped (truncated=true and \
                       turned_away= on the seed), and unexpanded= on a seed the callers whose own callers its walk \
                       did not follow, at the 3-hop bound or left by the cap, so its reach counts none beyond them. \
                       Callers through dynamic dispatch, function values and code outside \
                       the repository are not seen, so a small footprint is not proof the change is safe. Requires \
                       index_repo first."
    )]
    async fn impact_of_change(
        &self,
        Parameters(p): Parameters<ImpactOfChangeParams>,
    ) -> Result<String, String> {
        let limit = p.limit.map_or(CHANGE_LISTED, NonZeroUsize::get);
        let changed = changed_regions(p.regions, p.diff.as_deref())?;
        self.answer(move |indexed| {
            let regions: Vec<Region<'_>> = changed
                .iter()
                .map(|changed| Region::new(&changed.path, changed.lines.clone()))
                .collect();
            let limits = Limits::new(IMPACT_DEPTH).with_max_symbols(IMPACT_MAX_SYMBOLS);
            let change = indexed.engine.analyze_change(&regions, limits);
            Ok(render::change(regions.len(), &change, limit))
        })
        .await
    }

    #[tool(
        description = "Find definitions nothing in the repository calls or uses - candidates for deletion in a \
                       clean-up. Judges only what the graph can see used: functions, methods and macros through their \
                       calls, and Go types through their uses. Judges every indexed language unless narrowed with \
                       'languages'; path_prefix narrows it to a subtree, and on a large repository an unscoped call \
                       takes seconds, so scope it to the code you are cleaning. Returns an <unused> envelope: count= \
                       the definitions listed, total= with truncated=true when 'limit' (default 50) cut the list, \
                       live= the definitions judged and found used, then an <excluded/> record counting what was not \
                       judged, each only when it is not zero - test, generated, entry (main, registered handlers, \
                       constructors, operators), vendored, unobservable (fields, values, locals, and types outside \
                       Go), uncertain (a call of the name bound nothing certain, so it may reach this one), and \
                       dispatched (a method an interface, a trait or a parent can route a call to). Then one \
                       <symbol/> record per unused definition in path order: name, qn, kind, lang, loc \
                       (file:start-end), and name_used_elsewhere=true when calls or uses spell its name but bind to \
                       other definitions. A definition listed is a candidate, not a proof: a call through a function \
                       value or a callback, a caller outside the repository (another service, a script, reflection, \
                       a runtime calling by name) and dynamic dispatch are unseen - grep its name across the tree and \
                       check how it is registered or exported before deleting it. Requires index_repo first."
    )]
    async fn find_unused(
        &self,
        Parameters(p): Parameters<FindUnusedParams>,
    ) -> Result<String, String> {
        self.catalog.check(p.languages.as_deref())?;
        let catalog = Arc::clone(&self.catalog);
        self.answer(move |indexed| {
            let limit = p.limit.map_or(UNUSED, NonZeroUsize::get);
            let path_prefix = p.path_prefix.as_deref();
            let judged: Vec<_> = selected(indexed, &catalog, p.languages.as_deref())
                .into_iter()
                .map(|(family, graph)| (family, graph.dead_code(&[], path_prefix, limit)))
                .collect();
            let languages = echo(p.languages.as_deref());
            let ask = UnusedAsk {
                languages: &languages,
                path_prefix,
                limit,
            };
            Ok(render::unused(&ask, &judged))
        })
        .await
    }

    #[tool(
        description = "Trace who calls a symbol, transitively: walks the call graph in reverse from the named \
                       function or method, hop by hop. Answers \"what is affected if I change this function?\" before \
                       an edit, rename, or removal. Searches every indexed language unless narrowed with 'languages'; \
                       a name resolving in several languages yields one match per language. Returns a <callers> \
                       envelope of <match lang=..> records: each the resolved symbol with its loc, the caller count \
                       within the hops limit, a reach-band grade, and flat <ref hops=N> records (hops=1 = direct, \
                       caller-ward distance from the target) carrying the caller's loc, at= for the line of its call, \
                       and an explicit role= (\"consumer\" for a real caller, or test/entry/generated for a reach \
                       artifact); 'role' keeps the callers of one role - the root then echoes role= and each <match> \
                       counts its callers per role in consumers, tests, entries, and generated; weakly resolved edges \
                       are marked confidence=likely (module-structure inference) or confidence=guess (name-only \
                       match) - unmarked means certain. A <match> also counts what the walk left out, each only when \
                       it is not zero: passed_over= the other symbols the name selects (qualify the name to pick one), \
                       unexpanded= the callers at the hops limit whose own callers were not followed, \
                       below_confidence= the callers the floor dropped, and turned_away= with truncated=true when the \
                       node cap (500) stopped the walk. A match without callers states why: entry point, dead code, \
                       or dynamic dispatch for a function, a method, or a macro. For a field or an enum variant, it \
                       states that the graph records no call or other use of it, and that grep searches the text for \
                       its name. For any other kind, it states that no call names it, or that no call at the \
                       confidence floor names it, and that find_references lists its recorded uses. An unknown name \
                       answers with up to 5 close alternatives (did you mean). Calls through an interface may be \
                       missed - check find_implementations of the interface too. Requires index_repo first."
    )]
    async fn find_callers(
        &self,
        Parameters(p): Parameters<FindCallersParams>,
    ) -> Result<String, String> {
        self.catalog.check(p.languages.as_deref())?;
        let catalog = Arc::clone(&self.catalog);
        self.answer(move |indexed| {
            let limits = Limits::new(p.hops.map_or(CALLER_HOPS, NonZeroU32::get))
                .with_min_confidence(p.min_confidence.floor())
                .with_max_symbols(CALLERS_MAX_SYMBOLS);
            let families: Vec<&str> = indexed
                .engine
                .graphs()
                .map(|(family, _)| family)
                .filter(|family| catalog.selects(p.languages.as_deref(), family))
                .collect();
            let walks: Vec<_> = indexed
                .engine
                .graphs()
                .filter(|(family, _)| families.contains(family))
                .filter_map(|(family, graph)| Some((family, graph.callers(&p.name, limits)?)))
                .collect();
            let suggestions = if walks.is_empty() {
                suggestions(indexed, &families, &p.name)
            } else {
                Vec::new()
            };
            let languages = echo(p.languages.as_deref());
            let ask = CallersAsk {
                name: &p.name,
                languages: &languages,
                role: p.role.map(CallerRole::as_str),
                floored: p.min_confidence.floor() != 0,
            };
            Ok(render::callers(&ask, &walks, &suggestions))
        })
        .await
    }

    #[tool(
        description = "Trace what a symbol calls: walks the call graph forward from the named function or method, the \
                       forward half of find_callers. Answers \"what does X depend on, what runs when it runs?\" \
                       without reading its body and every body it calls. Searches every indexed language unless \
                       narrowed with 'languages'; a name resolving in several languages yields one match per language. \
                       Returns a \
                       <callees> envelope of <match lang=..> records: each the resolved symbol with its loc and the \
                       callee count within the hops limit (default 1, direct callees only), and flat <ref hops=N> \
                       records nearest first carrying the callee's loc, an explicit role= (\"consumer\", or \
                       test/entry/generated), confidence=likely / guess on a weakly resolved call, and, on a direct \
                       callee, at= for the line of the call. A <match> also counts what the walk left out, each only \
                       when it is not zero: unresolved= the calls in the bodies walked that bound no definition the \
                       graph holds (a library, a builtin, or a name the resolver could not tell apart - read the body \
                       for those), passed_over= the other symbols the name selects, unexpanded= the callees whose own \
                       callees were not followed, at the hops limit or left when the node cap stopped the walk, \
                       below_confidence= the callees the floor dropped, and turned_away= with truncated=true when the \
                       node cap (500) stopped the walk. A call \
                       through a function value, an interface or a virtual method binds only where the graph can name \
                       its target, so a callee missing here can still run. An unknown name answers with up to 5 close \
                       alternatives (did you mean). Requires index_repo first."
    )]
    async fn find_callees(
        &self,
        Parameters(p): Parameters<FindCalleesParams>,
    ) -> Result<String, String> {
        self.catalog.check(p.languages.as_deref())?;
        let catalog = Arc::clone(&self.catalog);
        self.answer(move |indexed| {
            let floor = p.min_confidence.floor();
            let limits = Limits::new(p.hops.map_or(CALLEE_HOPS, NonZeroU32::get))
                .with_min_confidence(floor)
                .with_max_symbols(CALLERS_MAX_SYMBOLS);
            let searched = selected(indexed, &catalog, p.languages.as_deref());
            let walks: Vec<_> = searched
                .iter()
                .filter_map(|&(family, graph)| {
                    graph.callees(&p.name, limits).map(|walk| (family, walk))
                })
                .collect();
            let suggestions = if walks.is_empty() {
                let families: Vec<&str> = searched.iter().map(|&(family, _)| family).collect();
                suggestions(indexed, &families, &p.name)
            } else {
                Vec::new()
            };
            let languages = echo(p.languages.as_deref());
            let ask = CallersAsk {
                name: &p.name,
                languages: &languages,
                role: None,
                floored: floor != 0,
            };
            Ok(render::callees(&ask, &walks, &suggestions))
        })
        .await
    }
}

#[tool_handler(router = self.tool_router)]
#[expect(
    clippy::unused_async_trait_impl,
    reason = "the SDK's tool_handler macro writes the handler's async methods"
)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "kartta answers structural questions about a code repository: where symbols are defined, who calls \
             them and what they call, where they are used, what implements an interface, which files import a file, \
             and what an edit would break. Call index_repo with the absolute repository root FIRST - it builds the \
             index in seconds and every other tool errors until then (a root given to the server at launch is \
             already building, and index_stats says when it is ready); one repository is indexed at a time. The \
             index holds a separate code graph per language; the symbol tools (find_symbols, read_definitions, \
             find_callers, find_callees, find_references, find_call_path, find_implementations, find_unused) \
             search every \
             indexed language by default and \
             take an optional 'languages' multiselect to narrow - the names are in the index_repo / index_stats \
             answer - and cross-language links (e.g. FFI) are not traced. The file tools (find_files, glob_files, \
             grep, list_dir, find_importers, file_outline, impact_of_edit, impact_of_change) span all languages \
             and take \
             repo-relative paths as returned by find_files / glob_files. An answer a cap, a depth limit or a filter \
             cut says how much it left out in its attributes. Intent map: locate a file by name -> find_files; list \
             files matching a glob -> glob_files; see what one directory contains -> list_dir; find lines of text \
             by regex / quote the exact code -> grep; where is X defined -> find_symbols; where in this known file \
             is X -> file_outline; read the source of X, Y, Z -> read_definitions; who calls X / what breaks if I \
             change it -> find_callers; what does X call -> find_callees; where is X used -> find_references; does \
             X reach Y through calls -> \
             find_call_path; what implements X -> find_implementations; what code does nothing call -> \
             find_unused; which files import this file -> \
             find_importers; how risky is this diff hunk or edit -> impact_of_edit; what does a whole diff of \
             several hunks reach -> impact_of_change; what languages and how much \
             code -> index_stats.",
        );
        info.server_info = Implementation::new("kartta-mcp", env!("CARGO_PKG_VERSION"));
        info
    }
}

/// The regions of an `impact_of_change` call: `regions` in order, then the hunks of `diff`.
fn changed_regions(
    regions: Vec<crate::params::ChangedRegion>,
    diff: Option<&str>,
) -> Result<Vec<Changed>, String> {
    let mut changed = Vec::with_capacity(regions.len());
    for (at, region) in regions.into_iter().enumerate() {
        let start = region.start_line.map_or(1, NonZeroU32::get);
        let end = region.end_line.map_or(u32::MAX, NonZeroU32::get);
        if end < start {
            return Err(format!(
                "regions[{at}]: end_line {end} is before start_line {start}"
            ));
        }
        changed.push(Changed {
            path: region.file,
            lines: start..=end,
        });
    }
    if let Some(diff) = diff {
        let regions = changed.len();
        diff::regions(diff, &mut changed)?;
        if changed.len() == regions {
            return Err(
                "diff holds no hunk under a +++ file header - pass git diff output unedited, hunk bodies included"
                    .to_owned(),
            );
        }
    }
    if changed.is_empty() {
        return Err("pass at least one region, or a diff".to_owned());
    }
    Ok(changed)
}

/// `name` as a glob that matches it literally: each character the glob dialect gives a meaning to is escaped.
fn escape_glob(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for character in name.chars() {
        if matches!(character, '*' | '?' | '[' | ']' | '{' | '}' | '\\' | '!') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_escaped_name_matches_only_itself() {
        let filter = Filter::new([format!("**/{}", escape_glob("a[1]{b}*"))]).unwrap();

        assert!(filter.matches("x/a[1]{b}*"));
        assert!(!filter.matches("x/a1b"));
    }
}
