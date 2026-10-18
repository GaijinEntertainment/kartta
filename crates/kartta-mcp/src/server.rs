use std::fmt;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::sync::Arc;

use kartta::Limits;
use kartta_fs::{
    Case, Filter, FuzzyOptions, GlobOptions, GrepOptions, ListError, ListOptions, Patterns,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

use crate::config::Config;
use crate::languages::{Catalog, echo};
use crate::params::{
    CallerRole, FindCallersParams, FindFilesParams, GlobFilesParams, GrepParams, IndexRepoParams,
    ListDirParams,
};
use crate::render::{self, CallersAsk, Globbed, SUGGESTIONS, Suggestion};
use crate::state::{Indexed, Request, Slot, Status};

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

/// How many hops `find_callers` walks when the call names none.
const CALLER_HOPS: u32 = 3;

/// How many symbols a walk over callers reaches before it stops, the symbol it starts from included.
const CALLERS_MAX_SYMBOLS: usize = 500;

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
        Ok(render::index(&indexed))
    }

    #[tool(
        description = "Show what the index currently holds: repository root, total files, and per-language \
                       file/symbol/call/use/include counts, plus 'files_without_symbols' on any language whose files \
                       did not all yield symbols (an unsupported dialect or a failed parse) and 'unheld' on any whose \
                       claimed files are binary or unreadable - each absent when it is zero. Use it to recall which \
                       languages are present (the valid 'languages' values for the symbol tools), to gauge how much \
                       code each language has, to tell a genuinely empty answer from missing coverage, or to check \
                       whether a repository is indexed at all. While a build runs it answers status=\"building\" \
                       with the seconds so far. Returns the same <index> envelope index_repo produced. Takes no \
                       arguments."
    )]
    async fn index_stats(&self) -> Result<String, String> {
        match self.slot.status() {
            Status::Building { root, elapsed } => Ok(render::building(&root, elapsed)),
            Status::Ready(indexed) => Ok(render::index(&indexed)),
            Status::Empty | Status::Failed { .. } => {
                self.slot.current().map(|indexed| render::index(&indexed))
            }
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
             them, where they are used, what implements an interface, which files import a file, and what an edit \
             would break. Call index_repo with the absolute repository root FIRST - it builds the index in seconds \
             and every other tool errors until then (a root given to the server at launch is already building, and \
             index_stats says when it is ready); one repository is indexed at a time. The index holds a separate \
             code graph per language; the symbol tools (find_symbols, read_definitions, find_callers, \
             find_references, find_call_path, find_implementations) search every indexed language by default and \
             take an optional 'languages' multiselect to narrow - the names are in the index_repo / index_stats \
             answer - and cross-language links (e.g. FFI) are not traced. The file tools (find_files, glob_files, \
             grep, list_dir, find_importers, file_outline, impact_of_edit) span all languages and take \
             repo-relative paths as returned by find_files / glob_files. An answer a cap, a depth limit or a filter \
             cut says how much it left out in its attributes. Intent map: locate a file by name -> find_files; list \
             files matching a glob -> glob_files; see what one directory contains -> list_dir; find lines of text \
             by regex / quote the exact code -> grep; where is X defined -> find_symbols; where in this known file \
             is X -> file_outline; read the source of X, Y, Z -> read_definitions; who calls X / what breaks if I \
             change it -> find_callers; where is X used -> find_references; does X reach Y through calls -> \
             find_call_path; what implements X -> find_implementations; which files import this file -> \
             find_importers; how risky is this diff hunk or edit -> impact_of_edit; what languages and how much \
             code -> index_stats.",
        );
        info.server_info = Implementation::new("kartta-mcp", env!("CARGO_PKG_VERSION"));
        info
    }
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
