package wire

// ListParams are the params of the list method.
type ListParams struct {
	// Path is the directory to list; empty lists the root.
	Path string `json:"path,omitempty"`
	// Depth is how many levels to descend; 0 lists the whole subtree.
	Depth int `json:"depth,omitempty"`
	// Limit caps the entries answered; 0 sets no cap.
	Limit int `json:"limit,omitempty"`
	// Collapse are globs naming the directories listed without their entries.
	Collapse []string `json:"collapse,omitempty"`
	// Exclude are globs naming the entries left out.
	Exclude []string `json:"exclude,omitempty"`
}

// Listing is the answer of list.
type Listing struct {
	Path      string   `json:"path"`
	Entries   []Listed `json:"entries"`
	Truncated bool     `json:"truncated"`
	// Excluded counts the entries Exclude left out.
	Excluded int `json:"excluded"`
	// Total counts the entries the index holds under the directory, at every depth.
	Total int `json:"total"`
	// StoppedBuild is nil unless the answer reads a stopped build's index.
	StoppedBuild *StoppedBuild `json:"stopped_build,omitempty"`
}

// Listed is one entry of a listing.
type Listed struct {
	Path string `json:"path"`
	// Kind is file, dir, symlink or other.
	Kind   string  `json:"kind"`
	Size   *uint64 `json:"size,omitempty"`
	Binary *bool   `json:"binary,omitempty"`
	// Target is where a symlink points.
	Target string `json:"target,omitempty"`
	// Unlisted counts the entries below a directory the listing did not descend into.
	Unlisted *int `json:"unlisted,omitempty"`
}

// GlobParams are the params of the glob method.
type GlobParams struct {
	Globs []string `json:"globs"`
	// Exclude are globs naming the paths left out, a directory with everything beneath it.
	Exclude []string `json:"exclude,omitempty"`
	// Limit caps the paths answered; 0 sets no cap.
	Limit int `json:"limit,omitempty"`
}

// Glob is the answer of glob: the matching paths in index order.
type Glob struct {
	Paths     []string `json:"paths"`
	Truncated bool     `json:"truncated"`
	// Excluded counts the matching paths Exclude left out before the answer ended.
	Excluded int `json:"excluded,omitempty"`
	// StoppedBuild is nil unless the answer reads a stopped build's index.
	StoppedBuild *StoppedBuild `json:"stopped_build,omitempty"`
}

// FuzzyParams are the params of the fuzzy method.
type FuzzyParams struct {
	Query string `json:"query"`
	// Globs keep the paths they match; nil keeps every path.
	Globs []string `json:"globs,omitempty"`
	// Exclude are globs naming the paths left out, a directory with everything beneath it.
	Exclude []string `json:"exclude,omitempty"`
	// Limit caps the matches answered; 0 sets no cap.
	Limit int `json:"limit,omitempty"`
}

// Fuzzy is the answer of fuzzy: the best-ranked paths, best first.
type Fuzzy struct {
	Matches   []FuzzyMatch `json:"matches"`
	Truncated bool         `json:"truncated"`
	// Matched counts the paths that matched the query, those past the limit included; nil for an empty query, which
	// is not counted.
	Matched *int `json:"matched,omitempty"`
	// StoppedBuild is nil unless the answer reads a stopped build's index.
	StoppedBuild *StoppedBuild `json:"stopped_build,omitempty"`
}

// FuzzyMatch is a path a fuzzy query ranked.
type FuzzyMatch struct {
	Path  string `json:"path"`
	Score int    `json:"score"`
}

// GrepParams are the params of the grep method.
type GrepParams struct {
	// ID names the grep for a cancel, any value 0 included; nil leaves it unnamed. The kartta package passes it on as it
	// is.
	ID *uint64 `json:"id,omitempty"`
	// BindingID names the grep for a cancel ByBindingID, apart from ID: a binding over the wire names each call it
	// runs by one, so the IDs its callers give never stop a call of its own. The kartta package's Grep and Call set
	// it, in place of any the params give.
	BindingID       uint64   `json:"binding_id,omitempty"`
	Patterns        []string `json:"patterns"`
	CaseInsensitive bool     `json:"case_insensitive,omitempty"`
	// Globs keep the files they match; nil keeps every file.
	Globs []string `json:"globs,omitempty"`
	// Exclude are globs naming the files left out.
	Exclude []string `json:"exclude,omitempty"`
	// Context is how many lines around each match to answer.
	Context int `json:"context,omitempty"`
	// Limit caps the matching lines answered; 0 sets no cap.
	Limit int `json:"limit,omitempty"`
	// CountAll counts every matching line and file past the limit, into TotalLines and TotalFiles.
	CountAll bool `json:"count_all,omitempty"`
	// MaxLineBytes caps the bytes of each line answered; 0 answers lines whole.
	MaxLineBytes int `json:"max_line_bytes,omitempty"`
	// MaxFileBytes is the size past which a file is not searched; 0 searches every size.
	MaxFileBytes uint64 `json:"max_file_bytes,omitempty"`
}

// Grep is the answer of grep: the files holding a match with their lines, and the account of every candidate.
type Grep struct {
	Files []GrepFile `json:"files"`
	// Lines counts the matching lines answered.
	Lines      int  `json:"lines"`
	Truncated  bool `json:"truncated"`
	TotalLines *int `json:"total_lines,omitempty"`
	TotalFiles *int `json:"total_files,omitempty"`
	Cancelled  bool `json:"cancelled"`
	Candidates int  `json:"candidates"`
	Excluded   int  `json:"excluded"`
	Symlinks   int  `json:"symlinks"`
	Searched   int  `json:"searched"`
	Binary     int  `json:"binary"`
	Oversized  int  `json:"oversized"`
	// RuledOut counts the candidates the content index ruled out unread.
	RuledOut   int           `json:"ruled_out"`
	Unsearched int           `json:"unsearched"`
	Failures   []GrepFailure `json:"failures"`
	// StoppedBuild is nil unless the answer reads a stopped build's index.
	StoppedBuild *StoppedBuild `json:"stopped_build,omitempty"`
}

// GrepFile is a file holding a match, and its lines answered.
type GrepFile struct {
	Path  string     `json:"path"`
	Lines []GrepLine `json:"lines"`
}

// GrepLine is a matching line, or a line of context around one.
type GrepLine struct {
	Number int `json:"number"`
	// Kind is match or context.
	Kind string `json:"kind"`
	Text string `json:"text"`
	// Cut is set when MaxLineBytes cut the line.
	Cut *Cut `json:"cut,omitempty"`
}

// Cut counts the bytes left out before and after the part of a line answered.
type Cut struct {
	Before int `json:"before"`
	After  int `json:"after"`
}

// GrepFailure is a file grep could not search.
type GrepFailure struct {
	Path    string `json:"path"`
	Message string `json:"message"`
}
