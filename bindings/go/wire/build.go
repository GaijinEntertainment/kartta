package wire

// BuildOptions are the params of the build method. The zero value builds every language kartta ships, under the
// tree's own .gitignore files, with no content index and no cache.
type BuildOptions struct {
	// ID names the build for a cancel, any value 0 included; nil leaves it unnamed. The kartta package passes it on as
	// it is.
	ID *uint64 `json:"id,omitempty"`
	// BindingID names the build for a cancel ByBindingID, apart from ID: a binding over the wire names each call it
	// runs by one, so the IDs its callers give never stop a call of its own. The kartta package's Build and Call set
	// it, in place of any the params give.
	BindingID uint64 `json:"binding_id,omitempty"`
	// Languages are the languages to build graphs of, by the names Version lists; nil builds every one, and an empty
	// list none.
	Languages []string `json:"languages"`
	// Threads bounds the threads the index runs on, at least 1; nil takes every core, and 0 answers InvalidParams.
	Threads *int `json:"threads,omitempty"`
	// Ignore is IgnoreInTree, the default, or IgnoreNone.
	Ignore Ignore `json:"ignore,omitempty"`
	// PrunedNames changes the entry names the walk neither indexes nor descends, editor and OS clutter by default.
	PrunedNames *SetChange `json:"pruned_names,omitempty"`
	// BinaryExtensions changes the extensions that settle a file as binary unread.
	BinaryExtensions *SetChange `json:"binary_extensions,omitempty"`
	// ContentIndex builds a trigram index of file content, kept where it says.
	ContentIndex *ContentIndex `json:"content_index,omitempty"`
	// ContentCache is a directory per-file data is shared through across builds.
	ContentCache string `json:"content_cache,omitempty"`
	// ContentCacheTTLSecs is the seconds after the build that computed it began that a cache entry expires; 0 is a week.
	ContentCacheTTLSecs uint64 `json:"content_cache_ttl_secs,omitempty"`
	// Stamps keeps the time each file the build reads last changed, so that a refresh reads again only the files that
	// changed since and those a change reaches. It holds memory for as long as the index lives: eight bytes a file for
	// its time, and what each file's resolution asked of the tree, most of it, 32 MiB over a tree of 182,714 entries.
	// On unix the time is the inode change time, which no write can set back; elsewhere it is the modification time,
	// so an edit that keeps a file's size and sets its modification time back goes unseen. A file changed within two
	// seconds before the build began is read again by the next refresh whatever its stamp says. nil keeps none, unless
	// Watch is set, which keeps stamps, and Watch with false answers InvalidParams.
	Stamps *bool `json:"stamps,omitempty"`
	// Watch follows the tree through the operating system's change notifications: the library refreshes the index
	// from each change on a thread of its own, as Refresh does, until the next Build or Close, so no Refresh is needed
	// for a change the notifications report. Build answers once the index holds the changes reported while it was
	// built; one macOS or Windows reports late is taken in after. Stats.Watch says whether the index still follows the
	// tree, and why not.
	Watch bool `json:"watch,omitempty"`
}

// Watch says whether the index follows its tree, which decides whether an answer can be older than the tree.
type Watch struct {
	// Following reports that a watcher refreshes the index from each change the operating system reports. When it is
	// false, the index takes a change in only through Refresh or another Build.
	Following bool `json:"following"`
	// Reason says why the watch a build asked for is off: it could not start, or it stopped. Empty while the watch
	// follows the tree, and when the build asked for no watch.
	Reason string `json:"reason,omitempty"`
	// Restarts counts the watchers the watch started over a refresh of the whole tree, because the one before dropped
	// changes, stopped, met the inotify limit, or saw the root deleted or moved.
	Restarts int `json:"restarts,omitempty"`
}

// Ignore names which ignore rules a build applies.
type Ignore string

// The ignore rule sets.
const (
	// IgnoreInTree applies the .gitignore files inside the tree.
	IgnoreInTree Ignore = "in_tree"
	// IgnoreNone applies no ignore rule.
	IgnoreNone Ignore = "none"
)

// SetChange extends a built-in set or replaces it; set exactly one field.
type SetChange struct {
	Extend  []string `json:"extend,omitempty"`
	Replace []string `json:"replace,omitempty"`
}

// ContentIndex says where a build keeps its trigram index of file content.
type ContentIndex struct {
	// Storage is StorageMemory, StorageFile or StorageMapped.
	Storage Storage `json:"storage"`
	// Dir is the directory the file and mapped storages create their scratch files in; both require it, and the
	// memory storage refuses it.
	Dir string `json:"dir,omitempty"`
}

// Storage names where a content index lives.
type Storage string

// The content index storages.
const (
	StorageMemory Storage = "memory"
	StorageFile   Storage = "file"
	StorageMapped Storage = "mapped"
)

// CancelParams are the params of the cancel method. Naming no call stops every build in flight, running or waiting its
// turn, every grep and every refresh in flight.
type CancelParams struct {
	// Build is the ID of the build to stop; nil names none.
	Build *uint64 `json:"build,omitempty"`
	// Grep is the ID of the grep to stop; nil names none.
	Grep *uint64 `json:"grep,omitempty"`
	// Refresh is the ID of the refresh to stop; nil names none.
	Refresh *uint64 `json:"refresh,omitempty"`
	// By is the name of a call the IDs above are compared with: ByID, the default, or ByBindingID.
	By By `json:"by,omitempty"`
}

// By is the name of a build, a grep or a refresh that a cancel compares its IDs with. An ID by one name never stops a
// call by the other.
type By string

// The names a cancel compares its IDs with.
const (
	// ByID compares them with the ID a call's caller gives.
	ByID By = "id"
	// ByBindingID compares them with the BindingID a binding gives each call it runs.
	ByBindingID By = "binding_id"
)

// CancelAnswer is the answer of the cancel method.
type CancelAnswer struct {
	// Stopped reports whether the cancel stopped a call in flight. A named call that has not entered the library starts
	// stopped, unless cancels that stopped nothing named 1,024 distinct later ids of its kind by the same name before
	// it entered; a grep that had already opened its last file answers whole, with Cancelled false, and a refresh past
	// its walk finishes.
	Stopped bool `json:"stopped"`
}

// Stats is the answer of the build and stats methods.
type Stats struct {
	// Entries counts the entries the index holds, directories included.
	Entries int `json:"entries"`
	// Threads counts the threads the index runs on: the build's Threads, or the ambient pool's size when it named none.
	Threads   int             `json:"threads"`
	Languages []LanguageFiles `json:"languages"`
	Graphs    []GraphStats    `json:"graphs"`
	// ContentIndex is nil when the build made none.
	ContentIndex *ContentIndexStats `json:"content_index,omitempty"`
	// ContentIndexFailure says why the build made no content index though it was asked for one; a build of the handle
	// tries again, and a refresh does not.
	ContentIndexFailure string      `json:"content_index_failure,omitempty"`
	Errors              BuildErrors `json:"errors"`
	Watch               Watch       `json:"watch"`
	// StoppedBuild is nil unless the answer reads a stopped build's index, which leaves Languages and Graphs empty.
	StoppedBuild *StoppedBuild `json:"stopped_build,omitempty"`
}

// StoppedBuild says that a cancel stopped the last build, so the handle holds its index and no graph, and an answer
// that carries it lacks what that index lacks: a path it does not name may exist.
type StoppedBuild struct {
	// Entries counts the entries the index holds.
	Entries int `json:"entries"`
	// Unvisited counts the entries the walk listed and the cancel turned away: a floor on what the index lacks, since
	// one that is a directory holds more. 0 when the cancel turned nothing away.
	Unvisited int `json:"unvisited"`
}

// LanguageFiles counts what a build made of one language's files.
type LanguageFiles struct {
	Name   string `json:"name"`
	Family string `json:"family"`
	// Claimed counts the files the language was the first to claim, of those the ignore rules kept.
	Claimed int `json:"claimed"`
	// Read counts the claimed files read as text.
	Read int `json:"read"`
	// Cached counts the read files whose extraction the content cache answered.
	Cached int `json:"cached"`
	// Held counts the read files the family's graph holds.
	Held int `json:"held"`
}

// GraphStats counts what one family's graph holds.
type GraphStats struct {
	Name                string         `json:"name"`
	Files               int            `json:"files"`
	FilesWithoutSymbols int            `json:"files_without_symbols"`
	Symbols             int            `json:"symbols"`
	Kinds               map[string]int `json:"kinds"`
	Calls               int            `json:"calls"`
	LocalCalls          int            `json:"local_calls"`
	Uses                int            `json:"uses"`
	Reads               int            `json:"reads"`
	Writes              int            `json:"writes"`
	Updates             int            `json:"updates"`
	Types               int            `json:"types"`
	Includes            int            `json:"includes"`
}

// ContentIndexStats counts what a content index holds.
type ContentIndexStats struct {
	Files int `json:"files"`
	// Unindexed counts the text files the index holds nothing of, because their read failed; a grep that selects one
	// reads it.
	Unindexed int    `json:"unindexed,omitempty"`
	Trigrams  int    `json:"trigrams"`
	Bytes     uint64 `json:"bytes"`
}

// BuildErrors are the failures a build collected rather than stopped on: Total of them, the first of them Listed.
type BuildErrors struct {
	Total  int            `json:"total"`
	Listed []BuildFailure `json:"listed"`
}

// BuildFailure is one failure a build collected.
type BuildFailure struct {
	// Kind is walk, ignore, name, read, content_index or cache; a failure of any other kind stops the call and answers
	// its code.
	Kind string `json:"kind"`
	// Path is the path that failed, as the system spells it. One in the tree starts with the root as Open was given it.
	Path string `json:"path,omitempty"`
	// File is Path relative to the root, which RefreshParams.Paths takes. It is empty for the root itself and for a path
	// outside the tree, such as a cache directory, which Path tells apart.
	File    string `json:"file,omitempty"`
	Message string `json:"message"`
}

// Version is the answer of the version method.
type Version struct {
	ABI       uint32   `json:"abi"`
	Version   string   `json:"version"`
	Methods   []string `json:"methods"`
	Languages []string `json:"languages"`
}
