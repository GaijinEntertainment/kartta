package wire

// BuildOptions are the params of the build method. The zero value builds every language kartta ships, under the
// tree's own .gitignore files, with no content index and no cache.
type BuildOptions struct {
	// ID names the build for a cancel. The kartta package sets it.
	ID uint64 `json:"id,omitempty"`
	// Languages are the languages to build graphs of, by the names Version lists; nil builds every one, and an empty
	// list none.
	Languages []string `json:"languages"`
	// Threads bounds the threads the index runs on; 0 takes every core.
	Threads int `json:"threads,omitempty"`
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

// CancelParams are the params of the cancel method.
type CancelParams struct {
	// Build is the ID of the build to stop; 0 stops every build in flight, running or waiting its turn.
	Build uint64 `json:"build,omitempty"`
}

// CancelAnswer is the answer of the cancel method.
type CancelAnswer struct {
	// Stopped reports whether the cancel stopped a build in flight, running or waiting its turn. A named build that has
	// not entered the library starts stopped.
	Stopped bool `json:"stopped"`
}

// Stats is the answer of the build and stats methods.
type Stats struct {
	// Entries counts the entries the index holds, directories included.
	Entries   int             `json:"entries"`
	Languages []LanguageFiles `json:"languages"`
	Graphs    []GraphStats    `json:"graphs"`
	// ContentIndex is nil when the build made none.
	ContentIndex *ContentIndexStats `json:"content_index,omitempty"`
	Errors       BuildErrors        `json:"errors"`
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
	Includes            int            `json:"includes"`
}

// ContentIndexStats counts what a content index holds.
type ContentIndexStats struct {
	Files    int    `json:"files"`
	Trigrams int    `json:"trigrams"`
	Bytes    uint64 `json:"bytes"`
}

// BuildErrors are the failures a build collected rather than stopped on: Total of them, the first of them Listed.
type BuildErrors struct {
	Total  int            `json:"total"`
	Listed []BuildFailure `json:"listed"`
}

// BuildFailure is one failure a build collected.
type BuildFailure struct {
	// Kind is walk, ignore, name, read, content_index, cache or other.
	Kind    string `json:"kind"`
	Path    string `json:"path,omitempty"`
	Message string `json:"message"`
}

// Version is the answer of the version method.
type Version struct {
	ABI       uint32   `json:"abi"`
	Version   string   `json:"version"`
	Methods   []string `json:"methods"`
	Languages []string `json:"languages"`
}
