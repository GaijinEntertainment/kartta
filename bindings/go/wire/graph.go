package wire

// Matches is the answer of a query that names a symbol: one part for each graph the name selects a symbol in, or for
// the one graph the params' Lang names, in the order of the engine's graphs. Each part names its graph.
type Matches[T any] struct {
	Matches []T `json:"matches"`
	// ReadAs is the name the parts answer for when the params' qualified name selects no symbol as written in the
	// graphs asked: the name read with each "." as "::", or else each "::" as ".", as "Index.refresh" is read as Rust's
	// "Index::refresh". A name that selects as written is never read, and ReadAs is then empty, as it is for search,
	// dead_code and resolution. A reading the method then answers nothing for leaves Matches empty.
	ReadAs string `json:"read_as,omitempty"`
	// FromReadAs and ToReadAs are the readings of a call_path's From and To, each end read on its own.
	FromReadAs string `json:"from_read_as,omitempty"`
	ToReadAs   string `json:"to_read_as,omitempty"`
}

// WalkParams are the params of the callers and callees methods.
type WalkParams struct {
	// Name selects the symbol to start from: a bare or a qualified name.
	Name string `json:"name"`
	// PathPrefix keeps only the symbols in files whose root-relative path starts with it, as one file's path or a
	// directory's with its closing slash; it reaches one of several copies that share a qualified name. Empty looks
	// in every file.
	PathPrefix string `json:"path_prefix,omitempty"`
	// Lang names the graph to ask, by a language or its family; empty asks every graph.
	Lang string `json:"lang,omitempty"`
	// Depth is how many calls away the walk goes; 0 takes the method's default, 3 for callers and 1 for callees.
	Depth int `json:"depth,omitempty"`
	// MinConfidence is the least confidence, 0 to 100, a call needs to be taken.
	MinConfidence int `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols the walk reaches, its root included; 0 takes the default, 500.
	MaxSymbols int `json:"max_symbols,omitempty"`
}

// Walk is what a walk over calls reached in one graph, and what it left out.
type Walk struct {
	Graph string     `json:"graph"`
	Root  Definition `json:"root"`
	// PassedOver counts the other symbols the name selects, of which Root was chosen.
	PassedOver int `json:"passed_over"`
	// Candidates lists the most called of those, the most first, at most five; Count is each one's callers.
	Candidates []Candidate `json:"candidates,omitempty"`
	Reached    []Reached   `json:"reached"`
	// Truncated reports that the symbol cap stopped the walk.
	Truncated bool `json:"truncated"`
	// TurnedAway counts the symbols the cap kept out.
	TurnedAway int `json:"turned_away"`
	// Unexpanded counts the symbols at the depth bound whose own calls were not followed.
	Unexpanded int `json:"unexpanded"`
	// BelowConfidence counts the calls under the confidence floor.
	BelowConfidence int `json:"below_confidence"`
	// Unresolved counts the calls that bound no symbol the graph holds.
	Unresolved int `json:"unresolved"`
	// Ambiguous counts, for a walk over callers, the calls of the root's name that bound no definition because several
	// definitions of the name may be meant: any of them may call the root, and none is among Reached. It is the root's
	// alone, so every depth counts the same, and absent when 0.
	Ambiguous int `json:"ambiguous,omitempty"`
	// FileScope lists, for a walk over callers, the files whose top level calls or uses the root's name and sees the
	// root, in path order. BelowConfidence also counts the files the floor left out.
	FileScope []FileScope `json:"file_scope,omitempty"`
}

// FileScope is a file whose top level, outside every definition, calls or uses a name. The graph keeps only the names a
// top level spells, so it has no line.
type FileScope struct {
	File string `json:"file"`
	// Confidence is how the file sees the symbol: 80 in the same file, 75 in its package, 70 through an include.
	Confidence int `json:"confidence"`
}

// Candidate is a symbol a name also selects that the answer passed over, and the count the query ranked it by.
type Candidate struct {
	Symbol Definition `json:"symbol"`
	Count  int        `json:"count"`
}

// Reached is a symbol a walk reached, and the call that reached it.
type Reached struct {
	Symbol     Definition `json:"symbol"`
	Depth      int        `json:"depth"`
	Confidence int        `json:"confidence"`
	// Line is the line of the call.
	Line int `json:"line"`
}

// Definition is a symbol a graph holds, where it is defined.
type Definition struct {
	Name          string `json:"name"`
	QualifiedName string `json:"qualified_name"`
	Owner         string `json:"owner,omitempty"`
	// Kind is the short label of the symbol's kind: func, method, class, struct, ns, iface, var, const, enum, alias,
	// trait, macro, field, variant or type.
	Kind      string `json:"kind"`
	File      string `json:"file"`
	Line      int    `json:"line"`
	EndLine   int    `json:"end_line"`
	Signature string `json:"signature,omitempty"`
	Exported  bool   `json:"exported"`
	// Flags lists those set of test, entry, generated and vendored.
	Flags []string `json:"flags,omitempty"`
}

// CallPathParams are the params of the call_path method.
type CallPathParams struct {
	From string `json:"from"`
	// FromPathPrefix keeps the symbols From may select to the files whose root-relative path starts with it, as one
	// file's path or a directory's with its closing slash. Empty keeps every file.
	FromPathPrefix string `json:"from_path_prefix,omitempty"`
	To             string `json:"to"`
	// ToPathPrefix keeps the symbols To may select to the files whose root-relative path starts with it, as one
	// file's path or a directory's with its closing slash. Empty keeps every file.
	ToPathPrefix string `json:"to_path_prefix,omitempty"`
	Lang         string `json:"lang,omitempty"`
	// Depth is how many calls long a path may be; 0 takes the default, 8.
	Depth         int `json:"depth,omitempty"`
	MinConfidence int `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols the search reaches; 0 takes the default, 20,000.
	MaxSymbols int `json:"max_symbols,omitempty"`
}

// CallPath is what a search for a chain of calls between two symbols found in one graph, and what it left out.
type CallPath struct {
	Graph      string     `json:"graph"`
	From       Definition `json:"from"`
	To         Definition `json:"to"`
	Found      bool       `json:"found"`
	Hops       []Reached  `json:"hops"`
	Truncated  bool       `json:"truncated"`
	TurnedAway int        `json:"turned_away"`
	// AtDepthBound counts the symbols at the depth bound whose own calls were not followed.
	AtDepthBound    int        `json:"at_depth_bound"`
	BelowConfidence int        `json:"below_confidence"`
	PassedOver      PassedOver `json:"passed_over"`
	// Candidates lists the most called of each end's passed-over symbols; nil when neither end passed one over.
	Candidates *PathCandidates `json:"candidates,omitempty"`
}

// PathCandidates lists the most called of each end's passed-over symbols, the most first, at most five each;
// Count is each one's callers.
type PathCandidates struct {
	From []Candidate `json:"from"`
	To   []Candidate `json:"to"`
}

// PassedOver counts the other symbols each end's name selects.
type PassedOver struct {
	From int `json:"from"`
	To   int `json:"to"`
}

// ReferencesParams are the params of the references method.
type ReferencesParams struct {
	Name string `json:"name"`
	// PathPrefix keeps only the symbols in files whose root-relative path starts with it, as one file's path or a
	// directory's with its closing slash; it reaches one of several copies that share a qualified name. Empty looks
	// in every file.
	PathPrefix    string `json:"path_prefix,omitempty"`
	Lang          string `json:"lang,omitempty"`
	MinConfidence int    `json:"min_confidence,omitempty"`
	// Access keeps the uses that read ("read"), that set ("write") or that update ("update") the symbol, an update
	// counting as a read and a write, and drops the calls; empty keeps every site.
	Access string `json:"access,omitempty"`
	// Limit caps the sites answered; 0 takes the default, 500.
	Limit int `json:"limit,omitempty"`
}

// References are the sites that call or use a symbol in one graph.
type References struct {
	Graph      string     `json:"graph"`
	Symbol     Definition `json:"symbol"`
	PassedOver int        `json:"passed_over"`
	// Candidates lists the most used of the symbols passed over, the most first, at most five; Count is each one's
	// sites.
	Candidates []Candidate `json:"candidates,omitempty"`
	Sites      []Site      `json:"sites"`
	// Total counts the sites that pass the confidence floor and the access kept, before the limit.
	Total           int `json:"total"`
	BelowConfidence int `json:"below_confidence"`
	// Counts counts the sites that pass the confidence floor by kind, whatever access is kept.
	Counts SiteCounts `json:"counts"`
	// Ambiguous counts the calls of the symbol's name that bound no definition because several definitions of the name
	// may be meant, and the uses of a member's name on a value whose type the graph could not follow, which bound no
	// definition: any of them may take this one.
	Ambiguous int `json:"ambiguous"`
}

// SiteCounts counts a symbol's sites by kind.
type SiteCounts struct {
	Calls   int `json:"calls"`
	Reads   int `json:"reads"`
	Writes  int `json:"writes"`
	Updates int `json:"updates"`
}

// Site is a call or a use of a symbol, and the definition it sits in.
type Site struct {
	SiteOf     Definition `json:"site_of"`
	Line       int        `json:"line"`
	Confidence int        `json:"confidence"`
	// Call reports a call rather than a use.
	Call bool `json:"call"`
	// Access is how the site touches the symbol: "read", "write" or "update"; a call, and a use as a type, read it.
	Access string `json:"access"`
}

// DefinitionsParams are the params of the definitions method.
type DefinitionsParams struct {
	Name string `json:"name"`
	Lang string `json:"lang,omitempty"`
	// From is the file the lookup asks from: definitions in its directory rank first.
	From string `json:"from,omitempty"`
	// Limit caps the definitions answered; 0 takes the default, 10.
	Limit int `json:"limit,omitempty"`
}

// SourcesParams are the params of the sources method: those of definitions, and the caps on each definition's text.
type SourcesParams struct {
	Name  string `json:"name"`
	Lang  string `json:"lang,omitempty"`
	From  string `json:"from,omitempty"`
	Limit int    `json:"limit,omitempty"`
	// MaxLines caps the lines of each definition, its doc comment included; 0 answers every line.
	MaxLines int `json:"max_lines,omitempty"`
	// MaxLineBytes caps the bytes of each line; 0 answers lines whole.
	MaxLineBytes int `json:"max_line_bytes,omitempty"`
}

// Definitions is the answer of definitions and sources in one graph.
type Definitions struct {
	Graph string `json:"graph"`
	// Total counts every definition the name selects, before the limit.
	Total int     `json:"total"`
	Found []Found `json:"found"`
}

// Found is a definition a name selects.
type Found struct {
	Definition Definition `json:"definition"`
	// Nearness is same_directory or elsewhere, from the file the lookup asks from.
	Nearness string `json:"nearness"`
	// Source is the definition's text, which only sources answers.
	Source *Source `json:"source,omitempty"`
	// Error is why sources could not read the definition's text.
	Error string `json:"error,omitempty"`
}

// Source is the text of a definition, its doc comment included, as its file holds it now.
type Source struct {
	DocLine   int    `json:"doc_line"`
	FirstLine int    `json:"first_line"`
	Lines     int    `json:"lines"`
	Text      string `json:"text"`
	// CutLines are the lines of Text the byte cap cut.
	CutLines []CutLine `json:"cut_lines"`
	// Resized reports that the file's size differs from the size the build read, so the lines may have moved.
	Resized bool `json:"resized"`
}

// CutLine is a line the byte cap cut, and its length before the cut.
type CutLine struct {
	Line  int `json:"line"`
	Bytes int `json:"bytes"`
}

// SearchParams are the params of the search method.
type SearchParams struct {
	Query string `json:"query"`
	Lang  string `json:"lang,omitempty"`
	// Limit caps the hits answered; 0 takes the default, 20.
	Limit int `json:"limit,omitempty"`
}

// Search is the answer of search in one graph.
type Search struct {
	Graph string `json:"graph"`
	Hits  []Hit  `json:"hits"`
	// Total counts the definitions that share a word with the query or that it names, before the limit.
	Total int `json:"total"`
	// Named counts those of Total whose name matched, rather than only their file's path.
	Named          int      `json:"named"`
	UnmatchedWords []string `json:"unmatched_words"`
	// DroppedWords counts the distinct words of the query past the first eight, which were not looked for.
	DroppedWords int `json:"dropped_words"`
	// FuzzyTotal counts what the fill-up by characters found before the limit, when it ran.
	FuzzyTotal *int `json:"fuzzy_total,omitempty"`
}

// Hit is a definition a search found.
type Hit struct {
	Definition Definition `json:"definition"`
	Score      float64    `json:"score"`
	// Exact reports that the name is the query as written.
	Exact bool `json:"exact"`
	// Fuzzy reports that the fill-up by characters found it.
	Fuzzy bool `json:"fuzzy"`
}

// ImplementationsParams are the params of the implementations method.
type ImplementationsParams struct {
	Name string `json:"name"`
	// PathPrefix keeps only the types in files whose root-relative path starts with it, as one file's path or a
	// directory's with its closing slash; it reaches one of several types that share a qualified name. Empty looks
	// in every file.
	PathPrefix string `json:"path_prefix,omitempty"`
	Lang       string `json:"lang,omitempty"`
	// Depth is how many levels below the root the search goes; 0 takes the default, 3.
	Depth int `json:"depth,omitempty"`
	// Limit caps the implementors answered; 0 takes the default, 100.
	Limit int `json:"limit,omitempty"`
}

// Implementations are the types that implement or extend a type in one graph.
type Implementations struct {
	Graph      string     `json:"graph"`
	Root       Definition `json:"root"`
	PassedOver int        `json:"passed_over"`
	// Candidates lists the most implemented of the types passed over, the most first, at most five; Count is each
	// one's direct implementors.
	Candidates []Candidate   `json:"candidates,omitempty"`
	Found      []Implementor `json:"found"`
	// Total counts the implementors within the depth bound, before the limit.
	Total      int `json:"total"`
	Unexpanded int `json:"unexpanded"`
}

// Implementor is a type that implements or extends the root, Depth levels below it.
type Implementor struct {
	Symbol Definition `json:"symbol"`
	Depth  int        `json:"depth"`
}

// OutlineParams are the params of the outline method.
type OutlineParams struct {
	File string `json:"file"`
}

// Outline is the answer of outline: the definitions of one file. Graph is the graph of the language that claims the
// file, empty when none does; when that graph does not hold the file, Held is false and the rest is empty.
type Outline struct {
	File    string       `json:"file"`
	Graph   string       `json:"graph,omitempty"`
	Held    bool         `json:"held"`
	Symbols []Definition `json:"symbols"`
}

// DependentsParams are the params of the dependents method.
type DependentsParams struct {
	File string `json:"file"`
	// Depth is how many includes away the search goes; 0 takes the default, 3.
	Depth int `json:"depth,omitempty"`
	// MaxFiles caps the files answered; nil takes the default, 100, and 0 sets no cap.
	MaxFiles *int `json:"max_files,omitempty"`
}

// Dependents is the answer of dependents: the files that include a file, directly or through others. Graph and Held
// are as Outline's.
type Dependents struct {
	File       string      `json:"file"`
	Graph      string      `json:"graph,omitempty"`
	Held       bool        `json:"held"`
	Found      []Dependent `json:"found"`
	Truncated  bool        `json:"truncated"`
	TurnedAway int         `json:"turned_away"`
	Unexpanded int         `json:"unexpanded"`
}

// Dependent is a file that includes the file asked about, Depth includes away.
type Dependent struct {
	File  string `json:"file"`
	Depth int    `json:"depth"`
}

// SegmentParams are the params of the analyze_segment method.
type SegmentParams struct {
	File string `json:"file"`
	// StartLine is the first line of the segment; 0 takes the first line of the file.
	StartLine int `json:"start_line,omitempty"`
	// EndLine is the last line of the segment; 0 takes the end of the file.
	EndLine int `json:"end_line,omitempty"`
	// Depth is how many calls away each symbol's blast goes; 0 takes the default, 3.
	Depth         int `json:"depth,omitempty"`
	MinConfidence int `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols each blast reaches; 0 takes the default, 500.
	MaxSymbols int `json:"max_symbols,omitempty"`
}

// Segment is the answer of analyze_segment: what a change to a range of a file's lines reaches. Graph and Held are as
// Outline's.
type Segment struct {
	File       string          `json:"file"`
	Graph      string          `json:"graph,omitempty"`
	Held       bool            `json:"held"`
	StartLine  int             `json:"start_line"`
	EndLine    int             `json:"end_line"`
	Symbols    []SegmentSymbol `json:"symbols"`
	Includes   []string        `json:"includes"`
	IncludedBy []string        `json:"included_by"`
}

// SegmentSymbol is a symbol the segment overlaps, its direct calls each way, and the callers it reaches.
type SegmentSymbol struct {
	Definition Definition  `json:"definition"`
	Callers    DirectCalls `json:"callers"`
	// Ambiguous counts the calls of its name that a walk over its callers counts as Walk.Ambiguous: any of them may
	// call it, and neither Callers nor Blast holds them. Absent when 0.
	Ambiguous  int         `json:"ambiguous,omitempty"`
	Callees    DirectCalls `json:"callees"`
	Unresolved int         `json:"unresolved"`
	Blast      Blast       `json:"blast"`
}

// DirectCalls are a symbol's direct callers or callees: at most 50 listed, and Total of them that pass the
// confidence floor.
type DirectCalls struct {
	Listed          []Reached `json:"listed"`
	Total           int       `json:"total"`
	BelowConfidence int       `json:"below_confidence"`
}

// Blast is what a walk over a symbol's callers reached.
type Blast struct {
	// Reach counts the callers reached, the symbol itself not among them.
	Reach int `json:"reach"`
	// ByDepth counts the callers reached at each depth, the direct callers first.
	ByDepth []int `json:"by_depth"`
	// AffectedFiles counts the other files that hold a caller reached or include the symbol's file.
	AffectedFiles int `json:"affected_files"`
	// Band ranks the reach: low, medium, high or critical.
	Band            string `json:"band"`
	Truncated       bool   `json:"truncated"`
	TurnedAway      int    `json:"turned_away"`
	Unexpanded      int    `json:"unexpanded"`
	BelowConfidence int    `json:"below_confidence"`
}

// ChangeParams are the params of the analyze_change method.
type ChangeParams struct {
	Regions []Region `json:"regions"`
	// Depth is how many calls away each changed symbol's walk goes; 0 takes the default, 3.
	Depth         int `json:"depth,omitempty"`
	MinConfidence int `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols each walk reaches; 0 takes the default, 500.
	MaxSymbols int `json:"max_symbols,omitempty"`
}

// Region is a changed range of a file's lines.
type Region struct {
	File string `json:"file"`
	// StartLine is the first line changed; 0 takes the first line of the file.
	StartLine int `json:"start_line,omitempty"`
	// EndLine is the last line changed; 0 takes the end of the file.
	EndLine int `json:"end_line,omitempty"`
}

// Change is the answer of analyze_change: what a change to regions across files reaches.
type Change struct {
	ChangedSymbols        int       `json:"changed_symbols"`
	ChangedFiles          int       `json:"changed_files"`
	UnheldRegions         int       `json:"unheld_regions"`
	RegionsOutsideSymbols int       `json:"regions_outside_symbols"`
	Footprint             int       `json:"footprint"`
	FootprintFiles        int       `json:"footprint_files"`
	TransitiveSum         int       `json:"transitive_sum"`
	Hotspots              []Hotspot `json:"hotspots"`
	TotalHotspots         int       `json:"total_hotspots"`
	Seeds                 []Seed    `json:"seeds"`
	TruncatedSeeds        int       `json:"truncated_seeds"`
}

// Hotspot is a symbol several changed symbols reach.
type Hotspot struct {
	Symbol Definition `json:"symbol"`
	// HitCount counts the changed symbols that reach it.
	HitCount int `json:"hit_count"`
	// MinDepth is how many calls away from the nearest of them it is.
	MinDepth int `json:"min_depth"`
	// AlsoChanged reports that it is itself a changed symbol.
	AlsoChanged bool `json:"also_changed"`
}

// Seed is a changed symbol and the callers it reaches.
type Seed struct {
	Symbol Definition `json:"symbol"`
	Reach  int        `json:"reach"`
	// Band ranks the reach: low, medium, high or critical.
	Band string `json:"band"`
	// CallerCount counts its direct callers that pass the confidence floor.
	CallerCount int `json:"caller_count"`
	// Ambiguous counts the calls of its name that a walk over its callers counts as Walk.Ambiguous: any of them may
	// call it, and neither CallerCount nor Reach counts them. Absent when 0.
	Ambiguous int `json:"ambiguous,omitempty"`
	// Consumers are those direct callers when there are 30 or fewer; empty otherwise.
	Consumers []Reached `json:"consumers"`
	// ConsumerDirs count those direct callers by directory, the most first, when there are more than 30.
	ConsumerDirs    []DirCount `json:"consumer_dirs"`
	Truncated       bool       `json:"truncated"`
	TurnedAway      int        `json:"turned_away"`
	Unexpanded      int        `json:"unexpanded"`
	BelowConfidence int        `json:"below_confidence"`
}

// DirCount counts the direct callers one directory holds.
type DirCount struct {
	Dir   string `json:"dir"`
	Count int    `json:"count"`
}
