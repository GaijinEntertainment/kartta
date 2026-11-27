use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, iter};

use kartta::{
    CancelToken, ChangeQuery, Graph, Hit, Limits, OutsideImplementations, Region, Search,
    SegmentQuery, Selector, SiteQuery, SourceOptions, SymbolKind, UnmatchedKind, ask_reading,
    is_parent, is_referent, selects_as_callers,
};
use kartta_fs::{
    Case, EntryKind, Filter, FuzzyOptions, GlobOptions, GrepOptions, Index, ListError, ListOptions,
    Patterns,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, tool, tool_handler, tool_router};

use crate::config::Config;
use crate::diff::{self, Changed};
use crate::languages::{CATALOG, echo};
use crate::llmxml::List;
use crate::params::{
    CallerRole, FileOutlineParams, FindCallPathParams, FindCalleesParams, FindCallersParams,
    FindFilesParams, FindImplementationsParams, FindImportersParams, FindReferencesParams,
    FindSymbolsParams, FindUnusedParams, GlobFilesParams, GrepParams, ImpactOfChangeParams,
    ImpactOfEditParams, IndexRootParams, IndexStatsParams, ListDirParams, Parameters,
    ReadDefinitionsParams, RefreshParams,
};
use crate::paths::{self, GLOB_CHARACTERS, Wanted, held_file, held_prefix, relative};
use crate::render::{
    self, CallersAsk, EditAsk, Found, Graphless, ImportersAsk, NameDefinitions, PathAsk, Read,
    SUGGESTIONS, Selection, SiteFilter, Suggestion, Unbuilt, Unmatched, UnusedAsk,
};
use crate::state::{GraphRow, INDEX_WAIT, Indexed, Request, Roots, Served, Unready};

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

/// How many paths `find_files` and `glob_files` list when the call names no cap.
const PATHS_LISTED: usize = 200;

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

/// How many includes away `find_importers` follows the files that include a file when the call names no hops.
const IMPORTER_HOPS: u32 = 3;

/// How many importers `find_importers` lists, nearest first, when the call names no limit.
const IMPORTERS_LISTED: usize = 200;

/// How many calls long a chain `find_call_path` looks for when the call names no bound.
const PATH_HOPS: u32 = 8;

/// How many symbols a call path search reaches before it stops.
const PATH_MAX_SYMBOLS: usize = 20_000;

/// How many calls away `impact_of_edit` walks the callers a change reaches, and the files that include the file.
const IMPACT_DEPTH: u32 = 3;

/// How many symbols the walk over a changed symbol's callers reaches before it stops.
const IMPACT_MAX_SYMBOLS: usize = 500;

/// How many symbols, and how many files of each include list, `impact_of_edit` lists when the call names no limit.
const IMPACT_LISTED: usize = 10;

/// How many of each side of a symbol's direct calls `impact_of_edit` reads from the engine.
const IMPACT_CALLS: usize = 50;

/// How many changed symbols, and how many hotspots, `impact_of_change` lists when the call names no limit.
const CHANGE_LISTED: usize = 20;

/// How many direct callers or users of a changed symbol `impact_of_change` lists before it counts them by directory.
const CHANGE_CONSUMERS: usize = 30;

/// How many changed symbols, and how many hotspots, `impact_of_change` reads from the engine.
const CHANGE_RANKED: usize = 300;

/// How many of the other symbols a name selects a symbol tool lists.
const CANDIDATES: usize = 5;

/// How many unused definitions `find_unused` lists when the call names no limit.
const UNUSED: usize = 50;

/// How many definitions `file_outline` lists, in source order, when the call names no limit.
const OUTLINE_LISTED: usize = 100;

/// How many hops `find_callers` walks when the call names none.
const CALLER_HOPS: u32 = 3;

/// How many symbols a walk over callers or callees reaches before it stops, the symbol it starts from included.
const CALLERS_MAX_SYMBOLS: usize = 500;

/// How many callers a `find_callers` match lists, nearest first, when the call names no limit.
const CALLERS_LISTED: usize = 100;

/// How many hops `find_callees` walks when the call names none: a body's callees fan out at every hop.
const CALLEE_HOPS: u32 = 1;

/// The MCP server: the tools, over each repository root it indexes.
#[derive(Clone)]
pub struct Server {
    roots: Arc<Roots>,
    config: Arc<Config>,
    /// How long `index_root` waits for its build before it answers that the build is still running.
    index_wait: Duration,
    tool_router: ToolRouter<Self>,
}

impl fmt::Debug for Server {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Server")
            .field("roots", &self.roots)
            .field("config", &self.config)
            .field("index_wait", &self.index_wait)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// A server whose builds take `config`, with nothing indexed.
    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self {
            roots: Arc::new(Roots::new(config.watch, config.max_roots)),
            config: Arc::new(config),
            index_wait: INDEX_WAIT,
            tool_router: Self::tool_router(),
        }
    }

    /// Starts building the index of `root` with every language, as `index_root` with no other argument does, and
    /// returns without waiting for it. A relative `root` is read against the working directory, and the root every call
    /// names is the absolute path it comes to. Until the build finishes, `index_stats` answers that it is building and
    /// every other query of the root answers an error saying so; an `index_root` of the same root joins it, and answers
    /// that it is still building once it has waited 30 s for it.
    ///
    /// The future it returns resolves once the build ends, to why it built no index, or `None` when it built one. The
    /// build runs whether or not the future is awaited.
    ///
    /// The build runs on the Tokio runtime's blocking threads, so this is called from within that runtime.
    pub fn index_at_launch(
        &self,
        root: PathBuf,
    ) -> impl Future<Output = Option<String>> + Send + 'static {
        let request = Request {
            root: std::path::absolute(&root).unwrap_or(root),
            languages: CATALOG.to_build(None),
            index_ignored: false,
        };
        let started = self.roots.start(request, self.config.build_options(false));
        async move {
            let mut outcome = match started {
                Ok(started) => started.outcome,
                Err(error) => return Some(error),
            };
            outcome.wait_for(Option::is_some).await.map_or_else(
                |_| Some("the build stopped before it answered".to_owned()),
                |built| {
                    built
                        .as_ref()
                        .and_then(|built| built.as_ref().err().cloned())
                },
            )
        }
    }

    /// Answers a query from the index of `root` on a blocking thread, and on the index's own threads.
    async fn answer<F>(&self, root: &str, answer: F) -> Result<String, String>
    where
        F: FnOnce(&Indexed) -> Result<String, String> + Send + 'static,
    {
        answer_from(self.roots.current(root)?, answer).await
    }
}

/// Answers a query from `served` on a blocking thread, and on the index's own threads.
async fn answer_from<F>(served: Arc<Served>, answer: F) -> Result<String, String>
where
    F: FnOnce(&Indexed) -> Result<String, String> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let indexed = served.read()?;
        indexed.engine.index().on_threads(|| answer(&indexed))
    })
    .await
    .map_err(|error| format!("the query stopped before it answered: {error}"))?
}

/// The `index_stats` answer of `indexed`, how the calls of each graph bind included when `coverage` is set.
fn stats(indexed: &Indexed, coverage: bool) -> String {
    let coverage = if coverage {
        indexed.engine.resolution(0)
    } else {
        Vec::new()
    };
    render::index(indexed, &coverage)
}

/// Awaits what `answering` answers, cancelling the token it was handed once `stop` resolves first. The answer is still
/// awaited after the cancel: work on a blocking thread ends only when it notices the token.
async fn stop_on<A: Future>(
    stop: impl Future<Output = ()>,
    answering: impl FnOnce(CancelToken) -> A,
) -> A::Output {
    let cancel = CancelToken::new();
    let mut answering = pin!(answering(cancel.clone()));
    tokio::select! {
        answered = &mut answering => answered,
        () = stop => {
            cancel.cancel();
            answering.await
        }
    }
}

/// A definition a name selects in one family's graph, and how near it sits.
type Nearest<'g> = (&'g str, kartta::Found<'g>);

/// The family graphs a query filtered by `languages` searches, in the engine's order.
fn selected<'g>(indexed: &'g Indexed, languages: Option<&[String]>) -> Vec<(&'g str, &'g Graph)> {
    indexed
        .engine
        .graphs()
        .filter(|(family, _)| CATALOG.selects(languages, family))
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

/// The tool error for a `parameter` whose name is blank: it names no definition, and a miss would suggest the shortest
/// names the index holds.
fn named(parameter: &str, name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(format!(
            "{parameter} is blank - pass a symbol's name; find_symbols finds one from a word of it"
        ));
    }
    Ok(())
}

/// Up to [`SUGGESTIONS`] close alternatives to `name` from the graphs a query searched, best first, none of them
/// named `name` itself.
fn suggestions<'g>(searched: &[(&'g str, &'g Graph)], name: &str) -> Vec<Suggestion<'g>> {
    let mut found: Vec<Suggestion<'g>> = searched
        .iter()
        .flat_map(|&(family, graph)| {
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

/// What a selector that picked no symbol in the graphs a query searched matches instead, among the definitions of
/// the kinds `selects` takes: what [`kartta::unmatched`] finds, or else the close alternatives a search finds.
fn unmatched<'g>(
    searched: &[(&'g str, &'g Graph)],
    selector: Selector<'_>,
    selects: fn(SymbolKind) -> bool,
) -> Unmatched<'g> {
    kartta::unmatched(searched.iter().copied(), selector, selects, SUGGESTIONS).map_or_else(
        || Unmatched::Suggested(suggestions(searched, selector.name)),
        Unmatched::Named,
    )
}

/// The types of the `searched` graphs that name `selector`'s name as a parent no graph holds a type of, at most
/// [`IMPLEMENTORS`] in all, those of the graphs that list none and hold no namesake left out; `instead` is what the
/// name names in place of
/// a type. Empty for a selector a path prefix or a line narrows, which look among the name's definitions, and for a
/// name some type of the graphs is named.
fn outside_graph<'g>(
    searched: &[(&'g str, &'g Graph)],
    selector: Selector<'_>,
    instead: Option<&kartta::Unmatched<'_>>,
) -> Vec<(&'g str, OutsideImplementations<'g>)> {
    let names_no_type = instead.is_none_or(|instead| {
        matches!(
            instead.kind,
            UnmatchedKind::OneEdit | UnmatchedKind::OtherKind
        )
    });
    if !names_no_type || selector.path_prefix.is_some() || selector.line.is_some() {
        return Vec::new();
    }
    let mut room = IMPLEMENTORS;
    let mut outside = Vec::new();
    for &(family, graph) in searched {
        let found = graph.implementations_outside(selector.name, room);
        if found.total > 0 || found.namesakes > 0 {
            room -= found.found.len();
            outside.push((family, found));
        }
    }
    if outside.iter().all(|(_, found)| found.total == 0) {
        return Vec::new();
    }
    outside
}

#[tool_router]
impl Server {
    #[tool(
        annotations(idempotent_hint = true),
        description = "Build the code index for a repository: walk the file tree and parse every recognized source \
                       file into a per-language code graph of symbols (functions, types, methods...) with their \
                       calls, uses and includes. Every other tool names the root it queries, and each but index_stats \
                       errors until that root's index exists. A build at 2 threads takes about 0.4 s on 900 files \
                       and about a minute on 170,000; a build still running after 30 s answers the \
                       status=\"building\" envelope of index_stats instead and runs on - call index_stats until it \
                       answers the index. The server holds the index of each root it builds, up to KARTTA_MAX_ROOTS \
                       (4 by default): a new root past it drops a failed root, else the least recently asked root not \
                       building, the launch root included, and the answer ends with a line naming the root dropped; \
                       building a held root with other languages or index_ignored replaces its index (a build of the \
                       same root, languages and index_ignored already under way is waited for, not \
                       repeated, and the index they built is answered as held, refreshed first while index_stats \
                       shows watch off). \
                       Returns an <index> envelope, its languages= \
                       naming the languages a 'languages' filter limited the build to (a symbol tool's answer for a \
                       name that matches nothing repeats them as index_languages=), its gitignore= naming the \
                       ignore files that applied as index_stats describes, its grep_index= present only when the \
                       configured trigram index could not be kept, saying why (grep then opens every candidate, and a \
                       new index_root tries again), with one <language/> record \
                       per built graph (files, symbols, calls, uses and includes, plus 'files_without_symbols' when a \
                       language's files did not all yield symbols and 'unheld' when some files it claims are binary or \
                       unreadable), and an <errors> block listing what the walk could not read - those language names \
                       are the valid values for the 'languages' filter of the symbol tools."
    )]
    async fn index_root(
        &self,
        Parameters(p): Parameters<IndexRootParams>,
    ) -> Result<String, String> {
        CATALOG.check(p.languages.as_deref())?;
        let root = PathBuf::from(&p.root);
        if !root.is_absolute() {
            return Err(format!(
                "the root {:?} is not an absolute path - pass the repository's absolute path",
                p.root
            ));
        }
        let root = tokio::task::spawn_blocking(move || match std::fs::read_dir(&root) {
            Ok(_) => Ok(root),
            Err(error) => Err(format!(
                "cannot index {}: {error} - pass the absolute path of a readable directory",
                root.display()
            )),
        })
        .await
        .map_err(|error| format!("the root check stopped before it answered: {error}"))??;
        let index_ignored = p.index_ignored.unwrap_or_default();
        let request = Request {
            root,
            languages: CATALOG.to_build(p.languages.as_deref()),
            index_ignored,
        };
        let started = self
            .roots
            .start(request.clone(), self.config.build_options(index_ignored))?;
        let mut outcome = started.outcome;
        let answer = async {
            let waited = tokio::time::timeout(self.index_wait, outcome.wait_for(Option::is_some))
                .await
                .map(|built| built.map(|built| built.clone()));
            let built = if let Ok(built) = waited {
                built
            } else {
                if let Some(building) = self.roots.building(&request) {
                    return Ok(render::unready(&building));
                }
                outcome
                    .wait_for(Option::is_some)
                    .await
                    .map(|built| built.clone())
            }
            .map_err(|_| "the build stopped before it answered".to_owned())?;
            let served = built.ok_or_else(|| "the build answered nothing".to_owned())??;
            answer_from(served, |indexed| Ok(render::index(indexed, &[]))).await
        }
        .await;
        let Some(dropped) = started.dropped else {
            return answer;
        };
        answer
            .map(|answer| format!("{answer}\n{dropped}"))
            .map_err(|error| format!("{error}\n{dropped}"))
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Show what the index holds: repository root, total files, and per-language \
                       file/symbol/call/use/include counts, plus 'files_without_symbols' on any language whose files \
                       did not all yield symbols (an unsupported dialect or a failed parse) and 'unheld' on any whose \
                       claimed files are binary or unreadable - each absent when it is zero. Use it to recall which \
                       languages are present (the valid 'languages' values for the symbol tools), to gauge how much \
                       code each language has, or to tell an empty answer from missing coverage. watch=\"following\" \
                       means the index takes in each edit the operating system reports; watch=\"off: <reason>\" means \
                       answers stay as the index last read the tree until refresh runs. gitignore=\"in-tree\" means \
                       the .gitignore files inside the root applied, and no other ignore file did \
                       ($GIT_DIR/info/exclude, a global excludes file); gitignore=\"ignored\" means none applied \
                       (index_ignored). A root inside a repository whose top is above it reads gitignore=\"in-tree: \
                       the root is inside the repository at <top>, whose .gitignore files above the root were not \
                       read\": the index can hold files those hide. pruned_worktrees=N, absent when zero, counts the \
                       linked worktrees of the root's repository inside the root, each listed as an empty directory: \
                       nothing below one is indexed. While a build runs it answers status=\"building\" with the \
                       seconds so far, and after a build that failed status=\"failed\" with its root and error; \
                       otherwise the <index> envelope index_root produced. Without a root it answers \
                       <roots total=\"N\"> around one such envelope per root the server holds, in the order it was \
                       first asked to build them, a root that cannot answer as status=\"broken\" with why, and a hint \
                       to call index_root when it holds none. \
                       With coverage=true it adds one <coverage/> record per language, to tell \"no callers\" from a \
                       language whose calls the graph binds poorly: sites, bound (split into certain / likely / \
                       guess), edges (caller-callee pairs), and why the other sites bound no definition, each only \
                       when it is not zero - external_import (an import of a module outside the repository), \
                       not_exported, undefined (a builtin or code outside the graph), not_admitted, ambiguous (several \
                       candidates, none nearer), past_bound (the graph stopped reading the receiver, at a cycle of \
                       declarations or a chain too long to follow), and through_receiver (of the undefined and not \
                       admitted, calls on a value of a type the graph does not define)."
    )]
    async fn index_stats(
        &self,
        Parameters(p): Parameters<IndexStatsParams>,
    ) -> Result<String, String> {
        let coverage = p.coverage.unwrap_or_default();
        let held = p
            .root
            .as_ref()
            .map_or_else(|| self.roots.listed(), |root| vec![self.roots.served(root)]);
        let mut answers = Vec::with_capacity(held.len());
        for held in held {
            answers.push(match held {
                Ok(served) => {
                    let asked = Arc::clone(&served);
                    match answer_from(served, move |indexed| Ok(stats(indexed, coverage))).await {
                        Ok(answer) => answer,
                        Err(error) if p.root.is_none() => render::broken(asked.root(), &error),
                        Err(error) => return Err(error),
                    }
                }
                Err(Unready::Unheld(miss)) => return Err(miss),
                Err(unready) => render::unready(&unready),
            });
        }
        Ok(if p.root.is_some() {
            answers.concat()
        } else {
            render::roots(answers)
        })
    }

    #[tool(
        annotations(idempotent_hint = true),
        description = "Bring the index up to date after files changed on disk. While index_stats shows \
                       watch=\"following\" the index does this by itself; call refresh when it shows watch off, or \
                       after a change made where notifications do not reach, such as a network mount or a container \
                       bind mount. Pass the repo-relative 'paths' that changed, with / between names, or omit them to \
                       check the whole tree; a path that is absolute, starts with a drive such as C:, or holds a \
                       backslash or a .. name is refused, and nothing is refreshed. Returns a <refresh> envelope: \
                       walked= the parts walked again (\"tree\" for all), then each only when it is not zero - added \
                       and removed files, extracted (files parsed again), resolved_again (other files whose calls or \
                       includes bind anew), dropped (files no graph holds now), reconfigured (language families \
                       resolved again because their go.mod or tsconfig.json changed what they resolve to, jsts for \
                       TypeScript and JavaScript), and widened= (the paths named) with parts= (the parts \
                       they came to) and limit= when those were more parts than it walks apart, so it walked the tree \
                       - the paths inside a directory the index lacks, such as an ignored one, are one part; \
                       elapsed_ms; and <errors> for what it could not read."
    )]
    async fn refresh(&self, Parameters(p): Parameters<RefreshParams>) -> Result<String, String> {
        let paths = p.paths.unwrap_or_default();
        let served = self.roots.current(&p.root)?;
        tokio::task::spawn_blocking(move || {
            served
                .refresh(paths.iter().map(String::as_str))
                .map(|refreshed| render::refresh(&refreshed))
        })
        .await
        .map_err(|error| {
            format!("the refresh stopped before it answered: {error} - call index_root to build the index again")
        })?
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Find files by name with a fuzzy match: pattern characters must appear in order but not \
                       contiguously, and filename matches rank above directory matches (\"serv\" finds \
                       pkg/service.go). Use it to locate a file when you know part of its name or path: a config file, \
                       a module, a handler. Covers ALL indexed files (configs, docs, assets), not just source code; \
                       optional include/exclude globs narrow the candidates first. To enumerate files by pattern alone \
                       - every .ts file, everything under src/ - use glob_files instead. Returns matching \
                       repo-relative paths, one per line, best first - exactly the path format the 'file' parameters \
                       of other tools expect; a path holding a control character such as a line break, or opening with \
                       a quote or a parenthesis, is written quoted with backslash escapes (\\n, \\t, \\\", \\\\), so \
                       no path reads as a closing line in parentheses. A list max_results (200 when omitted) cut has a \
                       (N of M matching paths, best first) line counting every match, and the default's cut ends with \
                       a line naming max_results."
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
        let includes = globs(p.includes.as_deref().unwrap_or_default())?;
        let excludes = globs(p.excludes.as_deref().unwrap_or_default())?;
        self.answer(&p.root, move |indexed| {
            let index = indexed.engine.index();
            let mut options = FuzzyOptions::default().limit(p.max_results.unwrap_or(PATHS_LISTED));
            if let Some(includes) = &includes {
                options = options.filter(includes);
            }
            if let Some(excludes) = &excludes {
                options = options.exclude(excludes);
            }
            let ranked = index.fuzzy(&p.pattern, &options);
            let mut paths = index.paths();
            let mut listed = List::default();
            for found in ranked.items() {
                listed.push(paths.get(found.position));
            }
            let includes = Selection {
                index,
                globs: p.includes.as_deref().unwrap_or_default(),
            };
            let cut = ranked
                .matched()
                .is_some_and(|matched| matched > listed.len());
            let mut answer = render::paths(&p.pattern, listed, ranked.matched(), &includes);
            if cut && p.max_results.is_none() {
                render::default_cut(&mut answer, PATHS_LISTED);
            }
            Ok(answer)
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "List indexed files matching glob patterns - pure enumeration: every .go file, everything under \
                       src/, the tree minus vendored code. Use it when the SET of files is the answer (inventory, \
                       scoping a sweep); when you are hunting ONE file by name, use find_files. Returns matching \
                       repo-relative paths, one per line, in sorted path order, quoted as find_files quotes them; a \
                       list max_results (200 when omitted) cut has a (truncated: ...) line, an (excludes removed: N) \
                       line counts the matches the excludes dropped before the list ended, and the default's cut ends \
                       with a line naming max_results."
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
        let excludes = globs(p.excludes.as_deref().unwrap_or_default())?;
        self.answer(&p.root, move |indexed| {
            let index = indexed.engine.index();
            let mut options = GlobOptions::default().limit(p.max_results.unwrap_or(PATHS_LISTED));
            if let Some(excludes) = &excludes {
                options = options.exclude(excludes);
            }
            let found = index.glob(&filter, &options);
            let globs = Selection {
                index,
                globs: &p.globs,
            };
            let mut answer = render::globbed(&globs, &found);
            if found.truncated() && p.max_results.is_none() {
                render::default_cut(&mut answer, PATHS_LISTED);
            }
            Ok(answer)
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Search file contents line by line with regular expressions - the evidence tool: pull the exact \
                       lines that prove how code behaves, find every textual occurrence of an identifier, string, or \
                       pattern. For symbol definitions use find_symbols, for call sites find_callers - grep finds \
                       text, not structure. Several patterns search as one pass (a line matching any of them is \
                       returned). Covers the indexed files; unscoped searches use the default exclusions documented by \
                       'excludes', and a search scoped with non-empty 'includes' and omitted 'excludes' covers exactly \
                       what it includes. Returns a <grep> envelope: one <file path=..> block per matched file with \
                       'NUMBER:text' match lines (and 'NUMBER-text' context when context_lines is set), at most \
                       max_results match lines (default 100) spent in file-path order. A line longer than 1024 bytes \
                       is cut around its match and marked [... omitted N bytes]. Binary files and files over 4 MiB are \
                       skipped and counted in binary= and oversized=; unreadable files are listed in <failures>, a \
                       path: error line each, quoted as find_files quotes a path, and a (+N more) line past 5. \
                       truncated=\"true\" means a match past the cap was seen, so more exist; unsearched= counts the \
                       files the answer does not cover once that match was found or a cancel stopped the search, \
                       excluded= the files the excludes dropped, ruled_out= the files the content index, built from \
                       their content as index_root read it, shows hold no match, and symlinks= the links selected, \
                       which are never read through. An answer with no truncated, unsearched= or <failures> holds \
                       every match in the files it did not skip as binary or oversized. The answer reads each file as \
                       it is now, but the set of files is the tree as the index last read it: a file added since is \
                       missing until a refresh."
    )]
    async fn grep(
        &self,
        Parameters(p): Parameters<GrepParams>,
        request: RequestContext<RoleServer>,
    ) -> Result<String, String> {
        if p.patterns.iter().all(|pattern| pattern.trim().is_empty()) {
            return Err("patterns needs at least one non-blank pattern".to_owned());
        }
        let case = if p.case_insensitive.unwrap_or(false) {
            Case::Insensitive
        } else {
            Case::Sensitive
        };
        let patterns = Patterns::new(&p.patterns, case).map_err(|error| error.to_string())?;
        let includes = globs(p.includes.as_deref().unwrap_or_default())?;
        let excludes = match &p.excludes {
            Some(excludes) => globs(excludes)?,
            None if includes.is_none() => {
                Some(Filter::new(GREP_EXCLUDES).map_err(|error| error.to_string())?)
            }
            None => None,
        };
        let context_lines = p.context_lines.unwrap_or_default();
        stop_on(request.ct.cancelled(), |cancel| {
            self.answer(&p.root, move |indexed| {
                let index = indexed.engine.index();
                let mut options = GrepOptions::default()
                    .context(render::grep_context(context_lines))
                    .limit(p.max_results.map_or(GREP_MATCHES, NonZeroUsize::get))
                    .max_line_bytes(GREP_LINE_BYTES)
                    .max_file_bytes(GREP_FILE_BYTES)
                    .cancel(&cancel);
                if let Some(includes) = &includes {
                    options = options.filter(includes);
                }
                if let Some(excludes) = &excludes {
                    options = options.exclude(excludes);
                }
                let matches = index.grep(&patterns, &options);
                let includes = Selection {
                    index,
                    globs: p.includes.as_deref().unwrap_or_default(),
                };
                Ok(render::grep(
                    &p.patterns,
                    &matches,
                    &includes,
                    context_lines,
                ))
            })
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "List a directory's entries from the index: subdirectories, files with their byte sizes \
                       (size=\"unknown\" when the file could not be read - distinct from size=\"0\", an empty file; \
                       directories and symlinks carry no size), and symlinks (marked explicitly, with their target). \
                       Use it when the SET of entries under one directory is the answer - what a package contains, \
                       whether a test or companion file exists beside a module. find_files hunts ONE file by name; \
                       glob_files enumerates by pattern across the whole tree. path \".\" (or omitted) lists the \
                       repository root; max_depth 1 (the default) shows immediate children only. Descent into vendored \
                       and generated trees (vendor, node_modules, dist, build, __pycache__) is suppressed by default - \
                       the directory entry itself still appears, and listing it by path shows its contents. Returns a \
                       <dir> envelope with one <entry/> record per entry in name-sorted order, a directory immediately \
                       preceding its children; a directory the listing did not enter (below max_depth or suppressed) \
                       carries unlisted= with the count of entries beneath it. Capped at max_entries (default 1000); \
                       total= on <dir> counts every entry the index holds under the path, and excluded= the entries \
                       skip_dotfiles dropped. An empty directory is a successful empty answer, not an error. The \
                       entries are the indexed universe - what the walk saw, not the live filesystem - while a file's \
                       size is read from the filesystem when the file is first listed."
    )]
    async fn list_dir(&self, Parameters(p): Parameters<ListDirParams>) -> Result<String, String> {
        let names = p
            .excludes
            .unwrap_or_else(|| LIST_COLLAPSED.map(str::to_owned).to_vec());
        let dotfiles = if p.skip_dotfiles.unwrap_or(false) {
            Some(Filter::new(["**/.*"]).map_err(|error| error.to_string())?)
        } else {
            None
        };
        let path = p.path.unwrap_or_default();
        let asked = relative(&path);
        let dir = match asked.trim_end_matches('/') {
            "" if asked.starts_with('/') => asked.to_owned(),
            "." => String::new(),
            dir => dir.to_owned(),
        };
        self.answer(&p.root, move |indexed| {
            let index = indexed.engine.index();
            let collapse = globs(&collapsed(index, &names)?)?;
            let mut options = ListOptions::default()
                .depth(p.max_depth.map_or(LIST_DEPTH, NonZeroUsize::get))
                .limit(p.max_entries.map_or(LIST_ENTRIES, NonZeroUsize::get));
            if let Some(collapse) = &collapse {
                options = options.collapse(collapse);
            }
            if let Some(dotfiles) = &dotfiles {
                options = options.exclude(dotfiles);
            }
            let listing = index
                .list(&dir, &options)
                .map_err(|error| match error {
                    ListError::NotFound { .. } => paths::unheld(index, &dir, Wanted::Dir),
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
        annotations(read_only_hint = true),
        description = "Find where symbols - functions, methods, types, constants, variables - are defined, by keyword \
                       search over the words of symbol names and of their files' paths. The starting point for \"where \
                       is X defined?\" and for pinning a symbol's exact name before find_callers / \
                       find_implementations. Returns a <symbols> envelope, one ranked <hit/> record per match: name, \
                       optional qn (the qualified name, when it says more), optional owner (the declaring type or \
                       namespace), kind, lang, loc, and a one-line sig. A hit whose name is the query outranks every \
                       other. To read a definition's source, pass its name to read_definitions. When keyword matches \
                       underfill the limit, fuzzy name matches top the answer up - those records carry fuzzy=true and \
                       are not matches for the query as requested. total= on <symbols> counts every match, and a limit \
                       that cuts the answer leaves a last line after the hits - a higher limit or a narrower query \
                       shows more; unmatched= names the query's words no definition holds, and then up to 5 \
                       <suggestion/> records before the hits list the definitions named one edit from the whole query \
                       - a character swapped with the next, dropped, or replaced or added by an ASCII letter, a digit, \
                       _ or a character of the query - which one_edit= counts; ask an unmatched word alone for those \
                       of that word. role= on a hit takes test, entry and generated in that precedence. Symbols in \
                       GENERATED or vendored code rank below hand-written ones. An answer with NO hits says so, and \
                       names any searched language holding files that yielded no symbols (an unsupported dialect or a \
                       failed parse) - read that as missing coverage rather than as the symbol not existing, and call \
                       index_stats for the per-language counts."
    )]
    async fn find_symbols(
        &self,
        Parameters(p): Parameters<FindSymbolsParams>,
    ) -> Result<String, String> {
        if p.query.trim().is_empty() {
            return Err(
                "query is blank - pass a word of a symbol's name or of its file's path".to_owned(),
            );
        }
        CATALOG.check(p.languages.as_deref())?;
        self.answer(&p.root, move |indexed| {
            let limit = p.limit.map_or(SYMBOLS, NonZeroUsize::get);
            let searched = selected(indexed, p.languages.as_deref());
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
            let one_edit = if unmatched.is_empty() {
                None
            } else {
                let query = Selector::new(&p.query, None);
                kartta::unmatched(searched.iter().copied(), query, |_| true, SUGGESTIONS)
                    .filter(|near| near.kind == UnmatchedKind::OneEdit)
            };
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
            let rows = if hits.is_empty() {
                indexed.graphs()
            } else {
                Vec::new()
            };
            let coverage: Vec<&GraphRow> = rows
                .iter()
                .filter(|row| row.files_without_symbols > 0)
                .filter(|row| searched.iter().any(|(family, _)| *family == row.family))
                .collect();
            let found = Found {
                hits,
                matched,
                unmatched,
                dropped_words,
                one_edit,
                built: indexed.languages.as_deref(),
            };
            Ok(render::hits(
                &p.query,
                p.languages.as_deref().map(echo).as_deref(),
                &found,
                &coverage,
            ))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Read the source of named definitions - functions, methods, types - several names in one call. \
                       Answers \"show me X\" without a search followed by a file read of a guessed range. Returns a \
                       <definitions> envelope with one <name of=.. total=..> per requested name, in request order; \
                       each holds its <definition> records nearest first, carrying name, optional qn, optional owner, \
                       kind, lang, loc, scope, and role=. scope is same-directory (beside 'from'), vendored, or other; \
                       it ranks by location and does not resolve which definition a given call binds to. The body is \
                       the file text from the doc block directly above the definition through its last line, each line \
                       numbered, at most 500 lines with each line cut at 1024 bytes; a cut line ends with [... omitted \
                       N bytes], and cut_lines= counts them. loc is the span the index last read and the body is read \
                       at call time, so a file edited since answers its new text at the old span, and resized=true \
                       marks a file whose size changed since. truncated=true marks a body cut before the span ends, or \
                       missing because the file no longer reaches the span; a definition whose file could not be read \
                       has no body, carries error= with the reason and its sig when one was captured. total on a \
                       <name> counts every definition the name selects, and a name nothing defines answers total=0; past the \
                       limit up to 5 <candidate/> records locate the next nearest, which path_prefix and line read."
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
        if p.names.iter().any(|name| name.trim().is_empty()) {
            return Err("names must not contain a blank name".to_owned());
        }
        CATALOG.check(p.languages.as_deref())?;
        self.answer(&p.root, move |indexed| {
            let limit = p.limit.map_or(DEFINITIONS, NonZeroUsize::get);
            let from = p
                .from
                .as_deref()
                .map(|from| held_file(indexed.engine.index(), from))
                .transpose()
                .map_err(|error| format!("from: {error}"))?;
            let searched = selected(indexed, p.languages.as_deref());
            let path_prefix = prefix_of(indexed, "path_prefix", p.path_prefix.as_deref())?;
            let line = p.line.map(NonZeroU32::get);
            let read = limit.saturating_add(CANDIDATES);
            let per_name: Vec<_> = p
                .names
                .iter()
                .map(|name| {
                    let written = Selector::new(name, path_prefix).with_line(line);
                    let (selecting, [read_as]) = ask_reading(
                        searched.iter().copied(),
                        [written],
                        |_| true,
                        |(family, graph), [named]| {
                            let definitions = graph.definitions(named, from, read);
                            (definitions.total > 0).then_some((family, definitions))
                        },
                    );
                    let total = selecting
                        .iter()
                        .map(|(_, definitions)| definitions.total)
                        .sum();
                    let mut found: Vec<Nearest<'_>> = selecting
                        .into_iter()
                        .flat_map(|(family, definitions)| {
                            definitions
                                .found
                                .into_iter()
                                .map(move |found| (family, found))
                        })
                        .collect();
                    found.sort_by_key(|(_, found)| found.nearness);
                    found.truncate(read);
                    let others = found.split_off(limit.min(found.len()));
                    (written, total, found, others, read_as)
                })
                .collect();
            let wanted: Vec<_> = per_name
                .iter()
                .flat_map(|(_, _, found, _, _)| found.iter().map(|(_, found)| found.definition))
                .collect();
            let options = SourceOptions::default()
                .max_lines(SOURCE_LINES)
                .max_line_bytes(SOURCE_LINE_BYTES);
            let mut sources = indexed.engine.sources(&wanted, options).into_iter();
            let answers: Vec<NameDefinitions<'_>> = per_name
                .into_iter()
                .map(
                    |(selector, total, found, others, read_as)| NameDefinitions {
                        selector,
                        read_as,
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
                        candidates: others
                            .into_iter()
                            .map(|(family, found)| (family, found.definition))
                            .collect(),
                        unmatched: if total == 0 {
                            unmatched(&searched, selector, |_| true)
                        } else {
                            Unmatched::Suggested(Vec::new())
                        },
                    },
                )
                .collect();
            Ok(render::definitions(
                p.languages.as_deref().map(echo).as_deref(),
                indexed.languages.as_deref(),
                &answers,
            ))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "List the types that implement or extend a named interface, base class, or trait, transitively. \
                       Use it when a contract changes and you need everything bound to it, or to find the real targets \
                       behind a dynamically-dispatched call site. Returns an <implementations> envelope, one <type/> \
                       record per implementor with its lang and loc, nearest first; depth=N marks one that implements \
                       the type through N-1 others. total= counts every implementor, at most 500 listed; passed_over= \
                       counts the other types the name selects, and then a <root/> names the type answered for and up \
                       to 5 <candidate/> records list the most implemented others. A type that implements an interface \
                       without naming it, as Go's structural typing allows, is not found. A name no indexed type has, \
                       as a library's Display or io.Reader, asked without path_prefix or line, lists under \
                       outside_graph=true the types whose written parents end with it, one hop, and namesakes= counts \
                       the other indexed types and namespaces of its last segment, which no listed parent reached. A \
                       name several types end with lists them instead; a name held only as kinds no type implements, \
                       such as a function, a variable or a Go defined type, counts them by kind and lists up to 5 of \
                       those definitions."
    )]
    async fn find_implementations(
        &self,
        Parameters(p): Parameters<FindImplementationsParams>,
    ) -> Result<String, String> {
        named("name", &p.name)?;
        CATALOG.check(p.languages.as_deref())?;
        self.answer(&p.root, move |indexed| {
            let searched = selected(indexed, p.languages.as_deref());
            let path_prefix = prefix_of(indexed, "path_prefix", p.path_prefix.as_deref())?;
            let written = Selector::new(&p.name, path_prefix)
                .with_line(p.line.map(NonZeroU32::get))
                .with_candidate_limit(CANDIDATES);
            let (mut found, [read_as]) = ask_reading(
                searched.iter().copied(),
                [written],
                is_parent,
                |(family, graph), [named]| {
                    Some((
                        family,
                        graph.implementations(named, u32::MAX, IMPLEMENTORS)?,
                    ))
                },
            );
            let mut room = IMPLEMENTORS;
            for (_, implementations) in &mut found {
                implementations.found.truncate(room);
                room -= implementations.found.len();
            }
            let named = written.with_name(read_as.as_deref().unwrap_or(&p.name));
            let languages = p.languages.as_deref().map(echo);
            let ask = CallersAsk {
                selector: written,
                read_as: read_as.as_deref(),
                languages: languages.as_deref(),
                role: None,
                built: indexed.languages.as_deref(),
            };
            if !found.is_empty() {
                return Ok(render::implementations(
                    &ask,
                    &found,
                    &Unmatched::Suggested(Vec::new()),
                ));
            }
            let instead =
                kartta::unmatched(searched.iter().copied(), named, is_parent, SUGGESTIONS);
            let outside = outside_graph(&searched, written, instead.as_ref());
            if !outside.is_empty() {
                return Ok(render::implementations_outside(&ask, &outside));
            }
            let unmatched = instead.map_or_else(
                || Unmatched::Suggested(suggestions(&searched, named.name)),
                Unmatched::Named,
            );
            Ok(render::implementations(&ask, &found, &unmatched))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Find every file that imports or includes a given file, directly or through a chain of imports \
                       (followed up to 'hops', default 3) - the downstream files of a shared module, header, or \
                       type-definition file. This is whole-file impact: for line-level impact use impact_of_edit, for \
                       a single function use find_callers. Returns an <importers> envelope, total= counting every \
                       downstream file, with one <file/> record per downstream file, nearest first and at most 'limit' \
                       (default 200); hops=N marks one that reaches the file through N-1 others, and unexpanded= \
                       counts the importers at the hops limit whose own importers were not followed, which a higher \
                       hops reaches. A Go file counts every importer of its package. For a file a code graph holds, a \
                       <note>, the whole text where no file imports it, says what the file's language counts as an \
                       importer: a Rust file counts the mod declarations that load it, not the files that use its \
                       items; for any other file the text says why no graph holds it."
    )]
    async fn find_importers(
        &self,
        Parameters(p): Parameters<FindImportersParams>,
    ) -> Result<String, String> {
        self.answer(&p.root, move |indexed| {
            let file = held_file(indexed.engine.index(), &p.file)?;
            let claimed = indexed.engine.graph_of(file);
            let hops = p.hops.map_or(IMPORTER_HOPS, NonZeroU32::get);
            let dependents = claimed.and_then(|(_, graph)| graph.dependents(file, hops, 0));
            let counted =
                claimed.and_then(|(family, graph)| Some((family, graph.conventions().includers?)));
            let graphless = Graphless::of(&indexed.engine, file, claimed.map(|(family, _)| family));
            let ask = ImportersAsk {
                file,
                listed: p.limit.map_or(IMPORTERS_LISTED, NonZeroUsize::get),
            };
            Ok(render::importers(
                &ask,
                graphless,
                dependents.as_ref(),
                counted,
            ))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "List the definitions of one known file in source order - \"where in this file is X?\" when the \
                       file is known but the place in it is not. Returns an <outline> envelope, total= counting every \
                       definition, with one <symbol/> record per definition, the first 'limit' (default 100) in source \
                       order: name; qn, the qualified name, omitted when it equals name; owner, the declaring type or \
                       namespace; in, on a definition without an owner inside another listed one - a local of a body, \
                       a member of an inline type or object literal, an item of a module block - the qn of the \
                       innermost one holding it, judged by lines, so one starting on its holder's first or last line, \
                       as in minified code, carries it only as a field or method of a callable's signature; a \
                       definition with neither starts outside every other listed one or on such a line; kind; \
                       lines=\"start-end\" (the definition's full span, or the one line of a one-line definition, \
                       nested definitions inside their enclosing one's span); and sig, the declaration, omitted when \
                       none was captured. Read the one span you need instead of the whole file or a guessed window. A \
                       file no code graph holds and a file with no recorded definition both report total=\"0\", told \
                       apart by the note. For callers and reach of a range use impact_of_edit."
    )]
    async fn file_outline(
        &self,
        Parameters(p): Parameters<FileOutlineParams>,
    ) -> Result<String, String> {
        self.answer(&p.root, move |indexed| {
            let file = held_file(indexed.engine.index(), &p.file)?;
            let claimed = indexed.engine.graph_of(file);
            let outline = claimed.and_then(|(_, graph)| graph.outline(file));
            let graphless = Graphless::of(&indexed.engine, file, claimed.map(|(family, _)| family));
            let listed = p.limit.map_or(OUTLINE_LISTED, NonZeroUsize::get);
            Ok(render::outline(file, graphless, outline.as_deref(), listed))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Find the shortest chain of calls from one symbol to another: from calls a symbol, which calls \
                       another, until one calls to. Answers \"does this entry point reach that function, and how?\" in \
                       one call, where following the calls by hand takes a read per hop. Returns a <call_path> \
                       envelope of one <match lang=..> per language the pair resolves in: from and to with from_loc \
                       and to_loc, hops= the chain's length, max_hops= the bound searched, and one <ref> per caller in \
                       call order - the last one calls to - each with loc= the caller's span, at= the line in the \
                       caller's own file where it calls the next symbol, role=, any confidence mark, and through= the \
                       interface, trait or base-class method a hop's call binds where it reaches the next method only \
                       through it, as find_callers marks it. When from and to resolve to one symbol, the match has \
                       hops=0 and no ref. A match without a chain carries found=false and no hops=, and states that an \
                       absent chain in the graph does not show one absent in the code; at_hops_bound= counts the \
                       callers at the hops limit a longer chain could pass through, and below_confidence= the callers \
                       the floor dropped; a match the floor left without a chain, in a search the node cap did not \
                       stop, says how many bind below it, which min_confidence \"any\" follows. A search the node cap \
                       (20000) stops carries turned_away=, the callers the cap turned away. passed_over_from= and \
                       passed_over_to= count the other symbols each name selects, the most called listed as up to 5 \
                       <candidate end=\"from\"|\"to\" callers=N/> records; only the picked pair is searched, which a \
                       match without a chain then says. With no match, an end whose name selects no symbol lists what \
                       it matches instead under <from> or <to>."
    )]
    async fn find_call_path(
        &self,
        Parameters(p): Parameters<FindCallPathParams>,
    ) -> Result<String, String> {
        named("from", &p.from)?;
        named("to", &p.to)?;
        CATALOG.check(p.languages.as_deref())?;
        self.answer(&p.root, move |indexed| {
            let hops = p.hops.map_or(PATH_HOPS, NonZeroU32::get);
            let floor = p.min_confidence.unwrap_or_default().floor();
            let limits = Limits::new(hops)
                .with_min_confidence(floor)
                .with_max_symbols(PATH_MAX_SYMBOLS);
            let searched = selected(indexed, p.languages.as_deref());
            let written = [
                Selector::new(
                    &p.from,
                    prefix_of(indexed, "from_path_prefix", p.from_path_prefix.as_deref())?,
                )
                .with_line(p.from_line.map(NonZeroU32::get))
                .with_candidate_limit(CANDIDATES),
                Selector::new(
                    &p.to,
                    prefix_of(indexed, "to_path_prefix", p.to_path_prefix.as_deref())?,
                )
                .with_line(p.to_line.map(NonZeroU32::get))
                .with_candidate_limit(CANDIDATES),
            ];
            let (paths, [from_read_as, to_read_as]) = ask_reading(
                searched.iter().copied(),
                written,
                |_| true,
                |(family, graph), [from, to]| Some((family, graph.call_path(from, to, limits)?)),
            );
            let from = written[0].with_name(from_read_as.as_deref().unwrap_or(&p.from));
            let to = written[1].with_name(to_read_as.as_deref().unwrap_or(&p.to));
            let unmatched_ends: Vec<_> = if paths.is_empty() {
                [("from", from), ("to", to)]
                    .into_iter()
                    .filter(|&(_, end)| !selects_as_callers(searched.iter().copied(), end))
                    .map(|(name, end)| (name, end, unmatched(&searched, end, |_| true)))
                    .collect()
            } else {
                Vec::new()
            };
            let selected_in: [Vec<&str>; 2] = if paths.is_empty() && unmatched_ends.is_empty() {
                [from, to].map(|end| {
                    searched
                        .iter()
                        .filter(|&&pair| selects_as_callers(iter::once(pair), end))
                        .map(|&(family, _)| family)
                        .collect()
                })
            } else {
                Default::default()
            };
            let languages = p.languages.as_deref().map(echo);
            let ask = PathAsk {
                from: &p.from,
                from_read_as: from_read_as.as_deref(),
                to: &p.to,
                to_read_as: to_read_as.as_deref(),
                languages: languages.as_deref(),
                hops,
                built: indexed.languages.as_deref(),
            };
            Ok(render::call_path(
                &ask,
                &paths,
                &selected_in,
                &unmatched_ends,
            ))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "List every site where a symbol is used: its calls, its uses as a type, and every read and write \
                       of a field, a constant or a variable. Answers \"where is this type, field, constant, or \
                       function used?\" and \"who sets Config.Timeout?\" (access=\"write\") in one call, without a \
                       text search that also matches strings, comments, and same-named symbols. Returns a <references> \
                       envelope of one <match lang=..> per language the name resolves in: the symbol's loc and kind, \
                       total= the sites it keeps, calls= types= reads= writes= updates= counting every site whatever \
                       access keeps, and files= the files holding them. The sites come in <file path= total=N> \
                       records, the most sites first: one <ref> per enclosing symbol and site kind in line order, with \
                       the enclosing symbol's name, qn and lines=, role=, site= (call, type, read - a member taken \
                       through a type, as EventType.Created, reads the type - write, or update for += and ++), at= the \
                       lines of that file where it touches the symbol, and any confidence mark. A caller is one site, \
                       at one of its calls; every other use is one site per line and kind. A match lists at most 100 \
                       sites, then names 50 more files with their total= alone (files_shown= when it names fewer than \
                       files=). file_scope=N counts the files whose top level names the symbol, each a <file_scope \
                       path=../> record after the files, the first 50 (file_scope_shown= past that), a member's marked \
                       guess; with no line kept, total= leaves them out, access does not filter them, and grep in the \
                       file finds the line. sites_under lists only the sites and file_scope files under a path prefix, \
                       the way past the files a match names; excluded=, files_excluded= and file_scope_excluded= count \
                       what it left out. A match names its language where that language records none of its symbol's \
                       uses, and no language records the reads of a body's local value, the uses as a value of a \
                       body's local function, or the uses as a type or a value of a body's local type. A method's \
                       calls through a value of its owner's type resolve only where the graph can name that type, so a \
                       missing site does not show an unused symbol. ambiguous= counts the calls of its name that bound \
                       no definition because several definitions may be meant, and the uses of a member's name on a \
                       value of a type the graph could not follow: any may be this symbol, and none is listed. \
                       below_confidence= counts the sites and file_scope files the floor dropped, and passed_over= the \
                       other symbols the name selects, the most used listed as up to 5 <candidate sites=N/> records. A \
                       match with no site and no file_scope record says what the graph does not record, or with \
                       below_confidence= how many uses bind below the floor, which min_confidence \"any\" lists. A \
                       name several symbols end with lists them instead, and a name held only as enum variants counts \
                       them and lists up to 5."
    )]
    async fn find_references(
        &self,
        Parameters(p): Parameters<FindReferencesParams>,
    ) -> Result<String, String> {
        named("name", &p.name)?;
        CATALOG.check(p.languages.as_deref())?;
        self.answer(&p.root, move |indexed| {
            let floor = p.min_confidence.unwrap_or_default().floor();
            let searched = selected(indexed, p.languages.as_deref());
            let path_prefix = prefix_of(indexed, "path_prefix", p.path_prefix.as_deref())?;
            let sites_under = prefix_of(indexed, "sites_under", p.sites_under.as_deref())?;
            let written = Selector::new(&p.name, path_prefix)
                .with_line(p.line.map(NonZeroU32::get))
                .with_candidate_limit(CANDIDATES);
            let (found, [read_as]) = ask_reading(
                searched.iter().copied(),
                [written],
                is_referent,
                |(family, graph), [named]| {
                    let query = SiteQuery::new(usize::MAX).with_min_confidence(floor);
                    let query = p
                        .access
                        .map_or(query, |access| query.with_access(access.access()));
                    Some((family, graph.conventions(), graph.references(named, query)?))
                },
            );
            let named = written.with_name(read_as.as_deref().unwrap_or(&p.name));
            let unmatched = if found.is_empty() {
                unmatched(&searched, named, is_referent)
            } else {
                Unmatched::Suggested(Vec::new())
            };
            let languages = p.languages.as_deref().map(echo);
            let ask = CallersAsk {
                selector: written,
                read_as: read_as.as_deref(),
                languages: languages.as_deref(),
                role: None,
                built: indexed.languages.as_deref(),
            };
            let listed = SiteFilter {
                one_access: p.access.is_some(),
                under: sites_under,
            };
            Ok(render::references(&ask, listed, &found, &unmatched))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Assess the blast radius of editing a span of lines - \"what breaks if I touch these lines?\" \
                       for a diff hunk under review or an edit you are about to make. For one named function use \
                       find_callers; for whole-file impact, find_importers. Returns an <impact> envelope with one \
                       <symbol> record per definition overlapping the range, total= counting them, at most 'limit' \
                       (default 10) listed, those of widest reach in source order. Each holds its caller and callee \
                       counts with sampled <ref/> lists, each ref with role=, any confidence mark, at= the line of the \
                       call or the use (in the caller's or the user's own file, and in this file for a callee), and \
                       through= as find_callers marks it; total= on <callers> and <callees> counts every one where a \
                       list shows at most 50, or fewer where a wide answer's size leaves less. ambiguous= counts the \
                       calls of its name that bound no definition because several definitions may be meant, any of \
                       which may call it and none of which callers= or the reach counts, and unresolved= the calls \
                       into code the graph does not hold, each only when it is not zero. A field, a variable or a \
                       constant carries users= and, in place of <callers>, a <users> list of the symbols that read, \
                       write or call it where its language records those uses (find_references lists every site). The \
                       reach counts the symbols across files that depend on it within 3 hops, a field's or a value's \
                       users and their callers included, graded in reach_band: low under 10, medium 10 to 49, high 50 \
                       to 199 or from 20 for an exported symbol, and critical from 200 or from 50 for an interface; \
                       turned_away= when the 500-symbol cap stopped the walk. <includes> and <included-by> count every \
                       file and list the first 'limit' in path order. A range covering only file-scope code (imports, \
                       comments) reports zero symbols and still lists the file's includes and includers; zero symbols \
                       means nothing is defined there, not that the edit is safe."
    )]
    async fn impact_of_edit(
        &self,
        Parameters(p): Parameters<ImpactOfEditParams>,
    ) -> Result<String, String> {
        let start_line = p.start_line.get();
        let end_line = p.end_line.map_or(u32::MAX, NonZeroU32::get);
        if end_line < start_line {
            return Err(format!(
                "end_line {end_line} is before start_line {start_line}"
            ));
        }
        self.answer(&p.root, move |indexed| {
            let limits = Limits::new(IMPACT_DEPTH).with_max_symbols(IMPACT_MAX_SYMBOLS);
            let query = SegmentQuery::new(limits).with_call_limit(IMPACT_CALLS);
            let file = held_file(indexed.engine.index(), &p.file)?;
            let claimed = indexed.engine.graph_of(file);
            let segment = claimed
                .and_then(|(_, graph)| graph.analyze_segment(file, start_line..=end_line, query));
            let graphless = Graphless::of(&indexed.engine, file, claimed.map(|(family, _)| family));
            let ask = EditAsk {
                file,
                lines: (start_line, end_line),
                listed: p.limit.map_or(IMPACT_LISTED, NonZeroUsize::get),
            };
            Ok(render::impact(&ask, graphless, segment.as_ref()))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Assess what a whole change reaches - a diff of several hunks across files and languages in one \
                       call: the symbols it touches, how far a change to each spreads through its callers (and from a \
                       field, a variable or a constant through the symbols that read or write it), and where those \
                       spreads meet. Use it to review a diff or plan a multi-file edit; for one span of one file with \
                       its callees and includes, use impact_of_edit. Pass the change as regions, as a unified diff, or \
                       both. Returns a <change> envelope: changed_symbols and changed_files, the footprint (every \
                       symbol the changed symbols' callers reach within 3 hops, each once) and footprint_files; a \
                       <seeds> list of the changed symbols, widest reach first and test/entry/generated code last, \
                       each <seed> with its loc, direct callers count, reach and reach_band, the reach's grade (low \
                       under 10, medium 10 to 49, high 50 to 199 or from 20 for an exported symbol, and critical from \
                       200 or from 50 for an interface), and its direct callers as <ref/> records with at= the line of \
                       the call in the caller's own file and through= as find_callers marks it (at most 10, in \
                       <callers total=>), or past 30 callers their directories (in <caller_dirs total=>) - a field, a \
                       variable or a constant lists its users= in <users> and <user_dirs> instead; and a <hotspots> \
                       list of the symbols two or more changed symbols reach, with reached_by= how many, hops= how \
                       near, and changed=true on one the change touches itself. <seeds> and <hotspots> each list at \
                       most 'limit' (default 20). A change that overlaps no symbol answers changed_symbols=0 with a \
                       note. Each count of what was left out appears only when it is not zero: unheld_regions= the \
                       regions in files no code graph holds, of them missing_regions= in a path the index does not \
                       hold, symlink_regions= in a symlink, special_regions= in a socket, a fifo or a device node, and \
                       unclaimed_regions= in a held file no language claims or declines, with a <declined language= \
                       regions=> per language that leaves such files out of its graph and an <unbuilt language= \
                       regions=> per language index_root's 'languages' left out; regions_outside_symbols= those \
                       overlapping no symbol; stopped_walks= the changed symbols whose walk the 500-symbol cap stopped \
                       (turned_away= on the seed); and on a seed unexpanded= the callers whose own callers its walk \
                       did not follow, and ambiguous= the calls of its name that bound no definition because several \
                       definitions may be meant, none of which its callers or its reach count. Callers through dynamic \
                       dispatch, function values and code outside the repository are not seen, so a small footprint is \
                       not proof the change is safe."
    )]
    async fn impact_of_change(
        &self,
        Parameters(p): Parameters<ImpactOfChangeParams>,
    ) -> Result<String, String> {
        let limit = p.limit.map_or(CHANGE_LISTED, NonZeroUsize::get);
        let regions = p.regions.unwrap_or_default();
        let named = regions.len();
        let changed = changed_regions(regions, p.diff.as_deref())?;
        self.answer(&p.root, move |indexed| {
            for (at, region) in changed.iter().take(named).enumerate() {
                held_file(indexed.engine.index(), &region.path)
                    .map_err(|error| format!("regions[{at}]: {error}"))?;
            }
            let regions: Vec<Region<'_>> = changed
                .iter()
                .map(|changed| Region::new(&changed.path, changed.lines.clone()))
                .collect();
            let limits = Limits::new(IMPACT_DEPTH).with_max_symbols(IMPACT_MAX_SYMBOLS);
            let query = ChangeQuery::new(limits)
                .with_consumer_limit(CHANGE_CONSUMERS)
                .with_seed_limit(CHANGE_RANKED)
                .with_hotspot_limit(CHANGE_RANKED);
            let change = indexed.engine.analyze_change(&regions, query);
            let unbuilt = Unbuilt::of(&indexed.engine, &regions, &change);
            Ok(render::change(regions.len(), &change, &unbuilt, limit))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Find definitions nothing in the repository calls or uses - candidates for deletion in a \
                       clean-up. Judges only what the graph can see used: functions, methods and macros through their \
                       calls, and Go types, fields, variables and constants through their uses. Judges every indexed \
                       language unless narrowed with 'languages'; path_prefix narrows it to a subtree, and on a large \
                       repository an unscoped call takes seconds, so scope it to the code you are cleaning. Returns an \
                       <unused> envelope: total= the unused definitions found, at most 'limit' (default 50) listed, \
                       live= the definitions judged and found used, then an <unjudged/> record counting what was not \
                       judged, each only when it is not zero - test, generated, entry (main, registered handlers, \
                       constructors, operators), vendored, unobservable (locals, and fields, values and types outside \
                       Go), uncertain (a call or a use of the name bound nothing certain, so it may reach this one), \
                       dispatched (a method an interface, a trait or a parent can route a call to), and reflected (no \
                       use reaches it, and reflection reads it by name, as encoding/json does a Go field its struct \
                       tag names). Then one <symbol/> record per unused definition in path order: name, qn, kind, \
                       lang, loc, and name_used_elsewhere=true when calls or uses spell its name but bind to other \
                       definitions. A definition listed is a candidate, not a proof: a call through a function value \
                       or a callback, a caller outside the repository (another service, a script, reflection, a \
                       runtime calling by name) and dynamic dispatch are unseen - grep its name across the tree and \
                       check how it is registered or exported before deleting it."
    )]
    async fn find_unused(
        &self,
        Parameters(p): Parameters<FindUnusedParams>,
    ) -> Result<String, String> {
        CATALOG.check(p.languages.as_deref())?;
        self.answer(&p.root, move |indexed| {
            let limit = p.limit.map_or(UNUSED, NonZeroUsize::get);
            let path_prefix = prefix_of(indexed, "path_prefix", p.path_prefix.as_deref())?;
            let judged: Vec<_> = selected(indexed, p.languages.as_deref())
                .into_iter()
                .map(|(family, graph)| (family, graph.dead_code(&[], path_prefix, limit)))
                .collect();
            let languages = p.languages.as_deref().map(echo);
            let ask = UnusedAsk {
                languages: languages.as_deref(),
                path_prefix,
                limit,
            };
            Ok(render::unused(&ask, &judged))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Trace who calls a symbol, hop by hop back from the named function or method. Answers \"what is \
                       affected if I change this function?\"; for a field's or a variable's reads and writes use \
                       find_references. Returns a <callers> envelope of one <match lang=..> per language the name \
                       resolves in: the symbol's loc, total= its callers within the hops limit, consumers=, tests=, \
                       entries= and generated= how many of them carry each role, each only when it is not zero, and \
                       flat <ref hops=N/> records, nearest first and within a hop consumer and entry code ahead of \
                       test and generated code, at most 'limit' (default 100), each with the caller's loc, at= the \
                       line of its call in the caller's own file, role= and any confidence mark. A caller whose call \
                       binds a same-named method of an interface, a trait or a base class of the method's owner \
                       carries through= that method's qualified name, so the call may run another implementation. \
                       file_scope=N counts the files whose top level calls or uses the symbol, each a <file_scope \
                       path=../> record after the refs, the first 50 by path (file_scope_shown= past that), with no \
                       line, not walked past, and not kept by 'role'. A call from the symbol's own body is one \
                       <self_call at=../> record, a line of the match's file, ahead of the refs whatever 'role' keeps; \
                       total= leaves it out. A <match> counts what the walk left out, each only when it is not zero: \
                       passed_over= the other symbols the name selects (the most called listed as up to 5 <candidate \
                       callers=N/> records), unexpanded= the callers whose own callers were not followed, at the hops \
                       limit or the node cap, ambiguous= the calls of its name that bound no definition because \
                       several definitions may be meant, any of which may call it, below_confidence= the callers and \
                       files the floor dropped, and turned_away= the callers the node cap (500) turned away. A match \
                       with no callers and no file_scope records says why in a note: entry point, dead code, or \
                       dynamic dispatch for a callable, find_references for a field, grep for an enum variant, and \
                       with below_confidence= how many bind below the floor, which min_confidence \"any\" lists. Calls \
                       through an interface may be missed - check find_implementations of the interface too."
    )]
    async fn find_callers(
        &self,
        Parameters(p): Parameters<FindCallersParams>,
    ) -> Result<String, String> {
        named("name", &p.name)?;
        CATALOG.check(p.languages.as_deref())?;
        self.answer(&p.root, move |indexed| {
            let limits = Limits::new(p.hops.map_or(CALLER_HOPS, NonZeroU32::get))
                .with_min_confidence(p.min_confidence.unwrap_or_default().floor())
                .with_max_symbols(CALLERS_MAX_SYMBOLS);
            let searched = selected(indexed, p.languages.as_deref());
            let path_prefix = prefix_of(indexed, "path_prefix", p.path_prefix.as_deref())?;
            let written = Selector::new(&p.name, path_prefix)
                .with_line(p.line.map(NonZeroU32::get))
                .with_candidate_limit(CANDIDATES);
            let (walks, [read_as]) = ask_reading(
                searched.iter().copied(),
                [written],
                |_| true,
                |(family, graph), [named]| Some((family, graph.callers(named, limits)?)),
            );
            let named = written.with_name(read_as.as_deref().unwrap_or(&p.name));
            let unmatched = if walks.is_empty() {
                unmatched(&searched, named, |_| true)
            } else {
                Unmatched::Suggested(Vec::new())
            };
            let languages = p.languages.as_deref().map(echo);
            let ask = CallersAsk {
                selector: written,
                read_as: read_as.as_deref(),
                languages: languages.as_deref(),
                role: p.role.map(CallerRole::as_str),
                built: indexed.languages.as_deref(),
            };
            let listed = p.limit.map_or(CALLERS_LISTED, NonZeroUsize::get);
            Ok(render::callers(&ask, &walks, &unmatched, listed))
        })
        .await
    }

    #[tool(
        annotations(read_only_hint = true),
        description = "Trace what a symbol calls: walks the call graph forward from the named function or method, the \
                       forward half of find_callers. Answers \"what does X depend on, what runs when it runs?\" \
                       without reading its body and every body it calls. Returns a <callees> envelope of one <match \
                       lang=..> per language the name resolves in: the symbol's loc, total= its callees within the \
                       hops limit, and flat <ref hops=N/> records nearest first, each with the callee's loc, role=, \
                       any confidence mark, and on a direct callee at= the line of the call in the match's file. A \
                       call from the symbol's own body is one <self_call at=../> record, a line of the match's file, \
                       ahead of the refs; total= leaves it out. A <match> counts what the walk left out, each only \
                       when it is not zero: unresolved= the calls in the bodies walked that bound no definition the \
                       graph holds (a library, a builtin, or a name the resolver could not tell apart - read the body \
                       for those), passed_over= the other symbols the name selects (the most called listed as up to 5 \
                       <candidate callers=N/> records), unexpanded= the callees whose own callees were not followed, \
                       at the hops limit or the node cap, below_confidence= the callees the floor dropped, and \
                       turned_away= the callees the node cap (500) turned away. A match without callees says that no \
                       other call in its body binds a definition the graph holds, or with below_confidence= how many \
                       bind below the floor, which min_confidence \"any\" lists. A call through a function value, an \
                       interface or a virtual method binds only where the graph can name its target, so a callee \
                       missing here can still run."
    )]
    async fn find_callees(
        &self,
        Parameters(p): Parameters<FindCalleesParams>,
    ) -> Result<String, String> {
        named("name", &p.name)?;
        CATALOG.check(p.languages.as_deref())?;
        self.answer(&p.root, move |indexed| {
            let limits = Limits::new(p.hops.map_or(CALLEE_HOPS, NonZeroU32::get))
                .with_min_confidence(p.min_confidence.unwrap_or_default().floor())
                .with_max_symbols(CALLERS_MAX_SYMBOLS);
            let searched = selected(indexed, p.languages.as_deref());
            let path_prefix = prefix_of(indexed, "path_prefix", p.path_prefix.as_deref())?;
            let written = Selector::new(&p.name, path_prefix)
                .with_line(p.line.map(NonZeroU32::get))
                .with_candidate_limit(CANDIDATES);
            let (walks, [read_as]) = ask_reading(
                searched.iter().copied(),
                [written],
                |_| true,
                |(family, graph), [named]| graph.callees(named, limits).map(|walk| (family, walk)),
            );
            let named = written.with_name(read_as.as_deref().unwrap_or(&p.name));
            let unmatched = if walks.is_empty() {
                unmatched(&searched, named, |_| true)
            } else {
                Unmatched::Suggested(Vec::new())
            };
            let languages = p.languages.as_deref().map(echo);
            let ask = CallersAsk {
                selector: written,
                read_as: read_as.as_deref(),
                languages: languages.as_deref(),
                role: None,
                built: indexed.languages.as_deref(),
            };
            Ok(render::callees(&ask, &walks, &unmatched))
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
             and what an edit would break. The server holds an index of each repository root it built, and every \
             tool names the absolute root it asks about: a root the server holds no index of is a tool error that \
             names the roots it holds. index_stats without a root shows each root held, built, building or failed. \
             Call index_root with the repository's absolute root when index_stats shows no index of it. An \
             index_root whose build runs past 30 s answers status=\"building\": call index_stats until it answers \
             the index. Each language has its own code \
             graph, and no tool follows a link from one language into another, such as a call through FFI. A path or \
             a path_prefix the index does not hold is a tool error that names what is wrong with it and the nearest \
             paths it holds. Every answer counts in one vocabulary: total= is every record an element counted, \
             shown= how many it lists where it lists fewer, truncated=\"true\" marks a list or a body that is \
             incomplete, and hops= a distance along calls or imports. \
             loc= is file:start-end, a definition's span, or file:line for one line, and lines= is that span without \
             the file, on a record inside a <file> or an answer of one file. role= marks a reach artifact as test, \
             entry or generated, and an ordinary caller, callee or site as \"consumer\". confidence=\"likely\" (bound \
             through the module layout) or \"guess\" (bound by the name alone) marks a weakly bound call or use, and \
             an unmarked one is certain. A name no definition carries answers up to 5 close alternatives (did you \
             mean): those named one edit from it, which one_edit= counts, or else the best search hits. \
             Intent map: locate a file by name -> find_files; list files matching a glob -> glob_files; see what one \
             directory contains -> list_dir; find lines of text by regex / quote the exact code -> grep; where is X \
             defined -> find_symbols; where in this known file is X -> file_outline; read the source of X, Y, Z -> \
             read_definitions; who calls X / what breaks if I change it -> find_callers; what does X call -> \
             find_callees; where is X used -> find_references; does X reach Y through calls -> find_call_path; what \
             implements X -> find_implementations; what code does nothing call -> find_unused; which files import \
             this file -> find_importers; how risky is this diff hunk or edit -> impact_of_edit; what does a whole \
             diff of several hunks reach -> impact_of_change; what languages and how much code -> index_stats; files \
             changed while index_stats shows watch off -> refresh.",
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
        let mut path = region.file;
        path.drain(..path.len() - relative(&path).len());
        changed.push(Changed {
            path,
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

/// The path prefix a call names in its parameter `parameter`, read as [`held_prefix`] reads it, or `None` when it names
/// none.
fn prefix_of<'p>(
    indexed: &Indexed,
    parameter: &str,
    prefix: Option<&'p str>,
) -> Result<Option<&'p str>, String> {
    prefix
        .map(|prefix| held_prefix(indexed.engine.index(), prefix))
        .transpose()
        .map_err(|error| format!("{parameter}: {error}"))
}

/// The globs that match the `list_dir` excludes `names` literally at any depth, or the tool error for the first name
/// that holds a glob character or a `\` and names no directory the index holds: such a name reads as a glob or as a
/// Windows path, and an exclude that excludes nothing says nothing.
fn collapsed(index: &Index, names: &[String]) -> Result<Vec<String>, String> {
    let marked = names
        .iter()
        .filter_map(|name| {
            name.chars()
                .find(|&character| escaped(character))
                .map(|character| (name.as_str(), character))
        })
        .collect();
    if let Some((name, character)) = unheld_dir(index, marked) {
        let fault = if character == '\\' {
            "holds a \\".to_owned()
        } else {
            format!("holds the glob character {character}")
        };
        return Err(format!(
            "excludes: {name:?} {fault} and names no indexed directory - list_dir excludes are directory names, \
             matched literally at any depth, such as \"vendor\" or \"pkg/deep\""
        ));
    }
    Ok(names.iter().map(|name| literal(name)).collect())
}

/// Whether the query glob dialect reads `character` as other than itself.
fn escaped(character: char) -> bool {
    character == '\\' || GLOB_CHARACTERS.contains(&character)
}

/// The glob that matches `name` literally at any depth, each character the dialect reads otherwise escaped.
fn literal(name: &str) -> String {
    let mut glob = String::from("**/");
    for character in name.chars() {
        if escaped(character) {
            glob.push('\\');
        }
        glob.push(character);
    }
    glob
}

/// The first of `names` that no directory the index holds bears, its path being the name or ending in a `/` and the
/// name, with the character it was paired with: at most one pass over the index's paths for all of them.
fn unheld_dir<'n>(index: &Index, mut names: Vec<(&'n str, char)>) -> Option<(&'n str, char)> {
    let mut paths = index.paths();
    for at in 0..index.len() {
        if names.is_empty() {
            return None;
        }
        if index.kind_at(at) == EntryKind::Dir {
            let path = paths.get(at);
            names.retain(|&(name, _)| {
                !path
                    .strip_suffix(name)
                    .is_some_and(|above| above.is_empty() || above.ends_with('/'))
            });
        }
    }
    names.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_name_matches_only_itself_at_any_depth() {
        let filter = Filter::new([literal("a!,c")]).unwrap();

        assert!(filter.matches("x/y/a!,c"));
        assert!(!filter.matches("x/a,c"));
    }

    #[test]
    fn a_literal_name_matches_each_glob_character_as_itself() {
        for name in ["de*p", "de?p", "de[e]p", "dee]p", "{deep}", "dee}p"] {
            let filter = Filter::new([literal(name)]).unwrap();

            assert!(filter.matches(&format!("x/{name}")), "{name}");
            assert!(!filter.matches("x/deep"), "{name}");
        }
    }

    /// Windows cannot spell a name that holds a `\`, so the case exists only where a filename may hold one.
    #[cfg(unix)]
    #[test]
    fn a_literal_name_matches_the_backslash_a_unix_name_holds() {
        let filter = Filter::new([literal(r"a\b")]).unwrap();

        assert!(filter.matches(r"x/y/a\b"));
        assert!(!filter.matches("x/ab"));
    }

    async fn until_cancelled(cancel: CancelToken) -> &'static str {
        for _ in 0..1000 {
            if cancel.is_cancelled() {
                return "stopped early";
            }
            tokio::task::yield_now().await;
        }
        "never cancelled"
    }

    /// Returns once `roots` holds an index of `root` to answer from.
    async fn indexed(roots: &Roots, root: &str) {
        while roots.current(root).is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Returns once `roots` builds `request`.
    async fn building(roots: &Roots, request: &Request) {
        while roots.building(request).is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn an_index_root_whose_build_outlasts_the_wait_answers_building_and_the_build_runs_on() {
        let first = tempfile::tempdir().unwrap();
        std::fs::write(first.path().join("a.go"), "package a\n\nfunc A() {}\n").unwrap();
        let second = tempfile::tempdir().unwrap();
        let mut server = Server::with_config(Config {
            watch: false,
            max_roots: NonZeroUsize::MIN,
            ..Config::default()
        });
        server.index_wait = Duration::from_millis(50);
        assert_eq!(server.index_at_launch(first.path().to_owned()).await, None);
        let held = server
            .roots
            .current(&first.path().display().to_string())
            .unwrap();
        let root = second.path().display().to_string();
        let params = IndexRootParams {
            root: root.clone(),
            languages: None,
            index_ignored: None,
        };

        let answer = tokio::time::timeout(
            Duration::from_secs(10),
            server.index_root(Parameters(params)),
        )
        .await
        .expect("index_root held the request for the whole build")
        .unwrap();
        drop(held);

        assert!(
            answer.starts_with("<index root=") && answer.contains(" status=\"building\" "),
            "{answer}"
        );
        let launched = first.path().display().to_string();
        assert!(
            answer.ends_with(&format!(
                "\nthis index_root dropped {launched:?} to stay within KARTTA_MAX_ROOTS=1 roots, the root a call \
                 named least recently - call index_root with root {launched:?} to build it again"
            )),
            "{answer}"
        );
        let ran_on =
            tokio::time::timeout(Duration::from_secs(10), indexed(&server.roots, &root)).await;
        assert!(ran_on.is_ok(), "the build stopped short of its index");
        assert_eq!(
            server.roots.current(&root).unwrap().read().unwrap().root,
            root
        );
    }

    #[tokio::test]
    async fn an_index_root_whose_build_is_replaced_names_the_root_it_dropped_beside_the_error() {
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let server = Server::with_config(Config {
            watch: false,
            max_roots: NonZeroUsize::MIN,
            ..Config::default()
        });
        assert_eq!(server.index_at_launch(first.path().to_owned()).await, None);
        let held = server
            .roots
            .current(&first.path().display().to_string())
            .unwrap();
        let go = vec!["go".to_owned()];
        let params = |languages: Option<&[String]>| IndexRootParams {
            root: second.path().display().to_string(),
            languages: languages.map(<[String]>::to_vec),
            index_ignored: None,
        };
        let request = |languages: Option<&[String]>| Request {
            root: second.path().to_owned(),
            languages: CATALOG.to_build(languages),
            index_ignored: false,
        };

        let (replaced, replacing, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                server.index_root(Parameters(params(None))),
                async {
                    building(&server.roots, &request(None)).await;
                    server.index_root(Parameters(params(Some(&go)))).await
                },
                async {
                    building(&server.roots, &request(Some(&go))).await;
                    drop(held);
                }
            )
        })
        .await
        .expect("an index_root never answered");

        let launched = first.path().display().to_string();
        assert_eq!(
            replaced,
            Err(format!(
                "a later index_root replaced this build before it finished\nthis index_root dropped {launched:?} to \
                 stay within KARTTA_MAX_ROOTS=1 roots, the root a call named least recently - call index_root with \
                 root {launched:?} to build it again"
            ))
        );
        assert!(
            replacing
                .as_ref()
                .is_ok_and(|answer| !answer.contains("dropped")),
            "the replacing call dropped no root: {replacing:?}"
        );
    }

    #[tokio::test]
    async fn a_listing_names_a_root_that_cannot_answer_beside_the_roots_that_do() {
        let trees = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let server = Server::with_config(Config {
            watch: false,
            ..Config::default()
        });
        let [first, second] = trees
            .each_ref()
            .map(|tree| tree.path().display().to_string());
        for (tree, root) in trees.iter().zip([&first, &second]) {
            std::fs::write(tree.path().join("a.go"), "package a\n\nfunc A() {}\n").unwrap();
            let params = IndexRootParams {
                root: root.clone(),
                languages: None,
                index_ignored: None,
            };
            server.index_root(Parameters(params)).await.unwrap();
        }
        server.roots.current(&first).unwrap().poison();
        let params = IndexStatsParams {
            root: None,
            coverage: None,
        };

        let listing = server.index_stats(Parameters(params)).await.unwrap();

        let broken = format!(
            "<roots total=\"2\">\n<index root={first:?} status=\"broken\">a refresh of {first} panicked"
        );
        assert!(listing.starts_with(&broken), "{listing}");
        assert!(
            listing.contains(&format!("\n<index root={second:?} files=\"1\" ")),
            "{listing}"
        );
    }

    #[tokio::test]
    async fn a_stop_cancels_the_answering_work_and_its_answer_still_arrives() {
        let answered = stop_on(std::future::ready(()), until_cancelled).await;

        assert_eq!(answered, "stopped early");
    }

    #[tokio::test]
    async fn a_stop_that_never_comes_leaves_the_work_uncancelled() {
        let cancelled = stop_on(std::future::pending(), |cancel| async move {
            cancel.is_cancelled()
        })
        .await;

        assert!(!cancelled);
    }
}
