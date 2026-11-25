package wire

import "encoding/json"

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
	// Unmatched says why Matches is empty when the name, or its reading, names definitions of the kinds the method
	// selects and picks none of them, names definitions none of which is of such a kind, or names no definition while
	// names one edit from it do; nil for a name that matched or for which none holds. callers, callees, references, implementations, definitions and sources set
	// it.
	Unmatched *Unmatched `json:"unmatched,omitempty"`
	// FromUnmatched and ToUnmatched say it for a call_path's From and To, each set only for an end that selects no
	// symbol.
	FromUnmatched *Unmatched `json:"from_unmatched,omitempty"`
	ToUnmatched   *Unmatched `json:"to_unmatched,omitempty"`
}

// Unmatched is what a name that selected no symbol names instead, among the definitions of the kinds the method
// selects.
type Unmatched struct {
	// Kind is "several" when several definitions under the path prefix end with the name and it picks none of them, a
	// longer name or a path prefix holding one picking one; "outside_prefix" when the name names definitions outside
	// the path prefix, none under it, or one that the selector looks at; "off_line" when it names definitions under
	// the path prefix that start off the line and at most one on it, these counting and listing those outside the
	// prefix too; "one_edit" when the name names no definition of any kind, and these are named one edit from it:
	// one character swapped with the next, dropped, replaced or added, a replacing or added one being an ASCII letter,
	// a digit, "_" or one the name holds; or "other_kind" when the name names definitions and none is of a kind the
	// method selects, these being every one of them, wherever it lies.
	Kind string `json:"kind"`
	// Total counts those definitions.
	Total int `json:"total"`
	// Listed is the first five of them.
	Listed []GraphDefinition `json:"listed"`
	// Kinds counts them by the label of their kind for "other_kind", and is nil for every other Kind.
	Kinds map[string]int `json:"kinds,omitempty"`
}

// GraphDefinition is a definition and the graph that holds it.
type GraphDefinition struct {
	Graph      string     `json:"graph"`
	Definition Definition `json:"definition"`
}

// WalkParams are the params of the callers and callees methods.
type WalkParams struct {
	// Name selects the symbol to start from: a bare or a qualified name.
	Name string `json:"name"`
	// PathPrefix keeps only the symbols in files whose root-relative path starts with it, as one file's path or a
	// directory's with its closing slash; it reaches one of several copies that share a qualified name. Empty looks
	// in every file.
	PathPrefix string `json:"path_prefix,omitempty"`
	// Line keeps only the symbols that start on it, one-based as a Definition's Line; it reaches one of several
	// symbols of the name in one file. nil looks at every line, and 0 selects no symbol.
	Line *int `json:"line,omitempty"`
	// Lang names the graph to ask, by a language or its family; empty asks every graph.
	Lang string `json:"lang,omitempty"`
	// Depth is how many calls away the walk goes, and 0 follows no call. It is required: nil answers InvalidParams.
	Depth *int `json:"depth,omitempty"`
	// MinConfidence is the least confidence, 0 to 100, a call needs to be taken.
	MinConfidence int `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols the walk reaches, its root included; 0 sets no cap.
	MaxSymbols int `json:"max_symbols,omitempty"`
	// CandidateLimit caps the Candidates answered; nil answers them all, and 0 answers PassedOver alone.
	CandidateLimit *int `json:"candidate_limit,omitempty"`
}

// Walk is what a walk over calls reached in one graph, and what it left out.
type Walk struct {
	Graph string     `json:"graph"`
	Root  Definition `json:"root"`
	// PassedOver counts the other symbols the name selects, of which Root was chosen.
	PassedOver int `json:"passed_over"`
	// Candidates lists the most called of those, the most first, at most CandidateLimit; Count is each one's callers.
	Candidates []Candidate `json:"candidates,omitempty"`
	Reached    []Reached   `json:"reached"`
	// Truncated reports that the symbol cap stopped the walk.
	Truncated bool `json:"truncated"`
	// TurnedAway counts the symbols the cap kept out.
	TurnedAway int `json:"turned_away"`
	// Unexpanded counts the symbols reached whose own calls were not followed: those at the depth bound, the root at a
	// depth of 0, and those the symbol cap left.
	Unexpanded int `json:"unexpanded"`
	// BelowConfidence counts the calls under the confidence floor.
	BelowConfidence int `json:"below_confidence"`
	// Unresolved counts the calls that bound no symbol the graph holds, a
	// construction of a class outside the graph as one.
	Unresolved int `json:"unresolved"`
	// Ambiguous counts, for a walk over callers, the calls of the root's name that bound no definition because several
	// definitions of the name may be meant: any of them may call the root, and none is among Reached. It is the root's
	// alone, so every depth counts the same, and absent when 0.
	Ambiguous int `json:"ambiguous,omitempty"`
	// FileScope lists, for a walk over callers, the files whose top level calls or uses the root's name and sees the
	// root, in path order. BelowConfidence also counts the files the floor left out.
	FileScope []FileScope `json:"file_scope,omitempty"`
	// Band ranks, for a walk over callers, how widely a change to the root reaches, from the callers in Reached: low,
	// medium, high or critical. A walk its Truncated or Unexpanded cut grades what it reached. Absent for a walk over
	// callees.
	Band string `json:"band,omitempty"`
	// SelfCall is the root's call of itself, which Reached does not list, since the walk starts at the root and reaches
	// each symbol once; absent when the root calls itself nowhere, or below the confidence floor, which BelowConfidence
	// then counts it under.
	SelfCall *SelfCall `json:"self_call,omitempty"`
}

// SelfCall is a call of a symbol from its own body: a recursion, or a call of another definition of its name that the
// graph bound to the symbol itself.
type SelfCall struct {
	// Confidence is how sure the resolution of the call is, 15 to 95.
	Confidence int `json:"confidence"`
	// Line is the one-based line of the call, in the symbol's own file; where the symbol calls itself from several
	// lines, the first line among the calls the most confident tier bound.
	Line int `json:"line"`
}

// FileScope is a file whose top level, outside every definition, calls or uses a name. The graph keeps only the names a
// top level spells, so it has no line.
type FileScope struct {
	File string `json:"file"`
	// Confidence is how the file sees the symbol: 80 in the same file, 75 in its package, 70 through an include; 85
	// where the top level reads the name through the body that types the value, as Ruby's pasted bodies do. In
	// References a member's file that sees it the first three ways is 15, a guess, as such a name keeps no receiver.
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
	// Through is, in a walk over callers, the method the call binds where it reaches the called method only through
	// it: a method of the same name that an interface, a trait or a base class of the called method's owner declares,
	// so the call runs whichever implementation the value holds. Nil for a call that binds the called symbol itself.
	Through *Definition `json:"through,omitempty"`
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
	// FromLine keeps the symbols From may select to those that start on it, one-based. nil keeps every line, and 0 selects
	// no symbol.
	FromLine *int   `json:"from_line,omitempty"`
	To       string `json:"to"`
	// ToPathPrefix keeps the symbols To may select to the files whose root-relative path starts with it, as one
	// file's path or a directory's with its closing slash. Empty keeps every file.
	ToPathPrefix string `json:"to_path_prefix,omitempty"`
	// ToLine keeps the symbols To may select to those that start on it, one-based. nil keeps every line, and 0 selects no
	// symbol.
	ToLine *int   `json:"to_line,omitempty"`
	Lang   string `json:"lang,omitempty"`
	// Depth is how many calls long a path may be, and 0 follows no call. It is required: nil answers InvalidParams.
	Depth         *int `json:"depth,omitempty"`
	MinConfidence int  `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols the search reaches; 0 sets no cap.
	MaxSymbols int `json:"max_symbols,omitempty"`
	// CandidateLimit caps the Candidates answered for each end; nil answers them all, and 0 answers PassedOver alone.
	CandidateLimit *int `json:"candidate_limit,omitempty"`
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
	// AtDepthBound counts the callers other than From the search reached at the depth bound, whose own callers it did
	// not look at: a longer path may pass through them. Unlike Unexpanded, it counts nothing the symbol cap left, which
	// Truncated reports.
	AtDepthBound    int        `json:"at_depth_bound"`
	BelowConfidence int        `json:"below_confidence"`
	PassedOver      PassedOver `json:"passed_over"`
	// Candidates lists the most called of each end's passed-over symbols; nil when neither end passed one over.
	Candidates *PathCandidates `json:"candidates,omitempty"`
}

// PathCandidates lists the most called of each end's passed-over symbols, the most first, at most the params'
// CandidateLimit each; Count is each one's callers.
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
	PathPrefix string `json:"path_prefix,omitempty"`
	// Line keeps only the symbols that start on it, one-based as a Definition's Line; it reaches one of several
	// symbols of the name in one file. nil looks at every line, and 0 selects no symbol.
	Line          *int   `json:"line,omitempty"`
	Lang          string `json:"lang,omitempty"`
	MinConfidence int    `json:"min_confidence,omitempty"`
	// Access keeps the uses that read ("read"), that set ("write"), that update ("update") or that name the symbol as a
	// type ("type"), and drops the calls; an update counts as a read and a write, and a use as a type as a read. Empty
	// keeps every site.
	Access string `json:"access,omitempty"`
	// Limit caps the sites answered, and 0 answers the counts alone. It is required: nil answers InvalidParams.
	Limit *int `json:"limit,omitempty"`
	// CandidateLimit caps the Candidates answered; nil answers them all, and 0 answers PassedOver alone.
	CandidateLimit *int `json:"candidate_limit,omitempty"`
}

// References are the sites that call or use a symbol in one graph.
type References struct {
	Graph      string     `json:"graph"`
	Symbol     Definition `json:"symbol"`
	PassedOver int        `json:"passed_over"`
	// Candidates lists the most used of the symbols passed over, the most first, at most the params' CandidateLimit;
	// Count is each one's sites.
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
	// FileScope lists the files whose top level calls or uses the symbol's name and sees the symbol, in path order. They
	// are no sites: the graph keeps no line for them and not how they touch the symbol, so neither Limit nor Access
	// bounds them. BelowConfidence also counts the files the floor left out.
	FileScope []FileScope `json:"file_scope,omitempty"`
	// TypeUsesUnrecorded is set when Symbol is a type and its graph records no use of a type where code names it as
	// one, such as a declaration, a signature, a generic argument or an import: Sites then hold only the calls that
	// name it and its reads as a value, so an empty Sites does not show that nothing uses the type.
	TypeUsesUnrecorded bool `json:"type_uses_unrecorded,omitempty"`
	// DataUsesUnrecorded is set when Symbol is a field, a variable or a constant of a kind whose reads and writes its
	// graph records nowhere, or a variable, a constant, a function or a type local to a function body, whose reads and
	// writes, a function's uses as a value and a type's uses as a type or a value among them, no graph records: Sites
	// then hold only uses that spell the name as something else, such as a call, never one of its reads or writes, so
	// an empty Sites does not show that nothing uses the symbol.
	DataUsesUnrecorded bool `json:"data_uses_unrecorded,omitempty"`
}

// SiteCounts counts a symbol's sites by kind.
type SiteCounts struct {
	Calls   int `json:"calls"`
	Reads   int `json:"reads"`
	Writes  int `json:"writes"`
	Updates int `json:"updates"`
	Types   int `json:"types"`
}

// Site is a call or a use of a symbol, and the definition it sits in.
type Site struct {
	SiteOf     Definition `json:"site_of"`
	Line       int        `json:"line"`
	Confidence int        `json:"confidence"`
	// Call reports a call rather than a use.
	Call bool `json:"call"`
	// Access is how the site touches the symbol: "read", "write", "update" or "type"; a call reads it.
	Access string `json:"access"`
}

// DefinitionsParams are the params of the definitions method.
type DefinitionsParams struct {
	Name string `json:"name"`
	Lang string `json:"lang,omitempty"`
	// From is the file the lookup asks from: definitions in its directory rank first.
	From string `json:"from,omitempty"`
	// Limit caps the definitions answered, and 0 answers the total alone. It is required: nil answers InvalidParams.
	Limit *int `json:"limit,omitempty"`
}

// SourcesParams are the params of the sources method: those of definitions, and the caps on each definition's text.
type SourcesParams struct {
	Name string `json:"name"`
	Lang string `json:"lang,omitempty"`
	From string `json:"from,omitempty"`
	// Limit caps the definitions answered, and 0 answers the total alone. It is required: nil answers InvalidParams.
	Limit *int `json:"limit,omitempty"`
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

// CutLine is a line the byte cap cut, and how many bytes of it the cut left out.
type CutLine struct {
	Line  int `json:"line"`
	Bytes int `json:"bytes"`
}

// SearchParams are the params of the search method.
type SearchParams struct {
	Query string `json:"query"`
	Lang  string `json:"lang,omitempty"`
	// Limit caps the hits answered, and 0 answers the counts alone. It is required: nil answers InvalidParams.
	Limit *int `json:"limit,omitempty"`
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
	// Line keeps only the types that start on it, one-based as a Definition's Line; it reaches one of several types
	// of the name in one file. nil looks at every line, and 0 selects no type.
	Line *int   `json:"line,omitempty"`
	Lang string `json:"lang,omitempty"`
	// Depth is how many levels below the root the search goes, and 0 finds none. It is required: nil answers
	// InvalidParams.
	Depth *int `json:"depth,omitempty"`
	// Limit caps the implementors answered, and 0 answers the total alone. It is required: nil answers InvalidParams.
	Limit *int `json:"limit,omitempty"`
	// CandidateLimit caps the Candidates answered; nil answers them all, and 0 answers PassedOver alone.
	CandidateLimit *int `json:"candidate_limit,omitempty"`
}

// Implementations are the types that implement or extend a type in one graph.
type Implementations struct {
	Graph      string     `json:"graph"`
	Root       Definition `json:"root"`
	PassedOver int        `json:"passed_over"`
	// Candidates lists the most implemented of the types passed over, the most first, at most the params'
	// CandidateLimit; Count is each one's direct implementors.
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
// file, empty when none does or when Missing or Directory is true; when that graph does not hold the file, Held is
// false and the rest is empty. Missing is true when the index holds no such path, Directory when it holds the path as
// a directory, whose files are the ones to ask about, and Special when it holds the path as a socket, a fifo or a
// device node, which no graph holds. Symlink names where a path the index holds as a symlink points, which no graph
// holds, and Declined names the language that leaves the held file out of its graph; a held file with neither Graph
// nor Declined is one no language claims.
type Outline struct {
	File      string       `json:"file"`
	Graph     string       `json:"graph,omitempty"`
	Held      bool         `json:"held"`
	Missing   bool         `json:"missing,omitempty"`
	Directory bool         `json:"directory,omitempty"`
	Special   bool         `json:"special,omitempty"`
	Symlink   *Symlink     `json:"symlink,omitempty"`
	Declined  *Declined    `json:"declined,omitempty"`
	Symbols   []Definition `json:"symbols"`
}

// MarshalJSON writes the outline as the wire answers it, with the symbols only where Held is true.
func (o Outline) MarshalJSON() ([]byte, error) {
	if !o.Held {
		return json.Marshal(fileState{o.File, o.Graph, o.Held, o.Missing, o.Directory, o.Special, o.Symlink, o.Declined})
	}
	type outline Outline
	return json.Marshal(outline(o))
}

// Symlink is where a held symlink points. Target is what the link stores, relative to its directory unless absolute,
// and empty when the walk could not read it. Path is the held entry the target names, as the index spells it, and
// Kind what that entry is, as Listed's Kind names it; both are empty when the target leaves the root, names nothing
// the index holds, or passes through a symlink on its way.
type Symlink struct {
	Target string `json:"target,omitempty"`
	Path   string `json:"path,omitempty"`
	Kind   string `json:"kind,omitempty"`
}

// Declined is the language that leaves a held file of its kind out of its graph, and the reason it states.
type Declined struct {
	Language string `json:"language"`
	Reason   string `json:"reason"`
}

// fileState is what an answer about one file holds when no graph holds the file.
type fileState struct {
	File      string    `json:"file"`
	Graph     string    `json:"graph,omitempty"`
	Held      bool      `json:"held"`
	Missing   bool      `json:"missing,omitempty"`
	Directory bool      `json:"directory,omitempty"`
	Special   bool      `json:"special,omitempty"`
	Symlink   *Symlink  `json:"symlink,omitempty"`
	Declined  *Declined `json:"declined,omitempty"`
}

// DependentsParams are the params of the dependents method.
type DependentsParams struct {
	File string `json:"file"`
	// Depth is how many includes away the search goes, and 0 finds none. It is required: nil answers InvalidParams.
	Depth *int `json:"depth,omitempty"`
	// MaxFiles caps the files answered, and 0 sets no cap. It is required: nil answers InvalidParams.
	MaxFiles *int `json:"max_files,omitempty"`
}

// Dependents is the answer of dependents: the files that include a file, directly or through others. Graph, Held,
// Missing, Directory, Special, Symlink and Declined are as Outline's.
type Dependents struct {
	File       string      `json:"file"`
	Graph      string      `json:"graph,omitempty"`
	Held       bool        `json:"held"`
	Missing    bool        `json:"missing,omitempty"`
	Directory  bool        `json:"directory,omitempty"`
	Special    bool        `json:"special,omitempty"`
	Symlink    *Symlink    `json:"symlink,omitempty"`
	Declined   *Declined   `json:"declined,omitempty"`
	Found      []Dependent `json:"found"`
	Truncated  bool        `json:"truncated"`
	TurnedAway int         `json:"turned_away"`
	Unexpanded int         `json:"unexpanded"`
	// Includers is what the graph records as including a file, in words that follow "counts" and call the file asked
	// about "this file", such as a Rust file's bodiless mod declarations and not the files that name its items in a
	// use. Found holds those, so a file that uses this one by another form is not among them. Empty where the graph's
	// language states none.
	Includers string `json:"includers,omitempty"`
}

// MarshalJSON writes the dependents as the wire answers them, with the files and counts only where Held is true.
func (d Dependents) MarshalJSON() ([]byte, error) {
	if !d.Held {
		return json.Marshal(fileState{d.File, d.Graph, d.Held, d.Missing, d.Directory, d.Special, d.Symlink, d.Declined})
	}
	type dependents Dependents
	return json.Marshal(dependents(d))
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
	// Depth is how many calls away each symbol's blast goes, and 0 follows no call. It is required: nil answers
	// InvalidParams.
	Depth         *int `json:"depth,omitempty"`
	MinConfidence int  `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols each blast reaches; 0 sets no cap.
	MaxSymbols int `json:"max_symbols,omitempty"`
	// CallLimit caps the direct callers, callees and users answered of each symbol, each side on its own; nil answers
	// them all, and 0 answers each side's Total alone.
	CallLimit *int `json:"call_limit,omitempty"`
}

// Segment is the answer of analyze_segment: what a change to a range of a file's lines reaches. Graph, Held, Missing,
// Directory, Special, Symlink and Declined are as Outline's. Where the params left EndLine at 0, it is the last line a
// symbol of the file reaches, and never before StartLine.
type Segment struct {
	File       string          `json:"file"`
	Graph      string          `json:"graph,omitempty"`
	Held       bool            `json:"held"`
	Missing    bool            `json:"missing,omitempty"`
	Directory  bool            `json:"directory,omitempty"`
	Special    bool            `json:"special,omitempty"`
	Symlink    *Symlink        `json:"symlink,omitempty"`
	Declined   *Declined       `json:"declined,omitempty"`
	StartLine  int             `json:"start_line"`
	EndLine    int             `json:"end_line"`
	Symbols    []SegmentSymbol `json:"symbols"`
	Includes   []string        `json:"includes"`
	IncludedBy []string        `json:"included_by"`
}

// MarshalJSON writes the segment as the wire answers it, with the lines, symbols and includes only where Held is true.
func (s Segment) MarshalJSON() ([]byte, error) {
	if !s.Held {
		return json.Marshal(fileState{s.File, s.Graph, s.Held, s.Missing, s.Directory, s.Special, s.Symlink, s.Declined})
	}
	type segment Segment
	return json.Marshal(segment(s))
}

// SegmentSymbol is a symbol the segment overlaps, its direct calls each way, and the callers it reaches.
type SegmentSymbol struct {
	Definition Definition  `json:"definition"`
	Callers    DirectCalls `json:"callers"`
	// Users are a field's, a variable's or a constant's direct users: the symbols that call it or whose code reads or
	// writes it, which Blast walks from as it walks from a function's callers. Absent for any other kind, and for one
	// with no user.
	Users *DirectCalls `json:"users,omitempty"`
	// Ambiguous counts the calls of its name that a walk over its callers counts as Walk.Ambiguous: any of them may
	// call it, and neither Callers nor Blast holds them. Absent when 0.
	Ambiguous  int         `json:"ambiguous,omitempty"`
	Callees    DirectCalls `json:"callees"`
	Unresolved int         `json:"unresolved"`
	Blast      Blast       `json:"blast"`
}

// DirectCalls are a symbol's direct callers or callees: at most the params' CallLimit listed, and Total of them that
// pass the confidence floor.
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
	// Depth is how many calls away each changed symbol's walk goes, and 0 follows no call. It is required: nil answers
	// InvalidParams.
	Depth         *int `json:"depth,omitempty"`
	MinConfidence int  `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols each walk reaches; 0 sets no cap.
	MaxSymbols int `json:"max_symbols,omitempty"`
	// ConsumerLimit is the most direct callers or users a changed symbol lists, past which it counts them by
	// directory; nil lists them all.
	ConsumerLimit *int `json:"consumer_limit,omitempty"`
	// SeedLimit caps the changed symbols answered; nil answers them all, and 0 answers ChangedSymbols alone.
	SeedLimit *int `json:"seed_limit,omitempty"`
	// HotspotLimit caps the hotspots answered; nil answers them all, and 0 answers TotalHotspots alone.
	HotspotLimit *int `json:"hotspot_limit,omitempty"`
}

// Region is a changed range of a file's lines.
type Region struct {
	File string `json:"file"`
	// StartLine is the first line changed; 0 takes the first line of the file.
	StartLine int `json:"start_line,omitempty"`
	// EndLine is the last line changed; 0 takes the end of the file.
	EndLine int `json:"end_line,omitempty"`
}

// Change is the answer of analyze_change: what a change to regions across files reaches. UnheldRegions counts the
// regions whose file no graph holds; of them, MissingRegions counts those whose path the index does not hold,
// SymlinkRegions those whose path it holds as a symlink, DirectoryRegions those whose path it holds as a directory,
// SpecialRegions those whose path it holds as a socket, a fifo or a device node, UnclaimedRegions those of held files
// no language claims or declines, and Declined those of held files a language leaves out of its graph. The rest are
// regions of held files the graph claiming them holds no record of.
type Change struct {
	ChangedSymbols        int               `json:"changed_symbols"`
	ChangedFiles          int               `json:"changed_files"`
	UnheldRegions         int               `json:"unheld_regions"`
	RegionsOutsideSymbols int               `json:"regions_outside_symbols"`
	Footprint             int               `json:"footprint"`
	FootprintFiles        int               `json:"footprint_files"`
	TransitiveSum         int               `json:"transitive_sum"`
	Hotspots              []Hotspot         `json:"hotspots"`
	TotalHotspots         int               `json:"total_hotspots"`
	Seeds                 []Seed            `json:"seeds"`
	TruncatedSeeds        int               `json:"truncated_seeds"`
	MissingRegions        int               `json:"missing_regions"`
	SymlinkRegions        int               `json:"symlink_regions"`
	DirectoryRegions      int               `json:"directory_regions"`
	SpecialRegions        int               `json:"special_regions"`
	UnclaimedRegions      int               `json:"unclaimed_regions"`
	Declined              []DeclinedRegions `json:"declined"`
}

// DeclinedRegions counts the regions of held files that one language leaves out of its graph for one reason.
type DeclinedRegions struct {
	Declined
	Regions int `json:"regions"`
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
	// UserCount counts a field's, a variable's or a constant's direct users that pass the confidence floor: the
	// symbols that call it or whose code reads or writes it, which the walk takes as a function's callers. Absent
	// when 0, and for any other kind.
	UserCount int `json:"user_count,omitempty"`
	// Ambiguous counts the calls of its name that a walk over its callers counts as Walk.Ambiguous: any of them may
	// call it, and neither CallerCount nor Reach counts them. Absent when 0.
	Ambiguous int `json:"ambiguous,omitempty"`
	// Consumers are its direct users for a field, a variable or a constant, and its direct callers for any other
	// kind, when there are the params' ConsumerLimit or fewer; empty otherwise.
	Consumers []Reached `json:"consumers"`
	// ConsumerDirs count those direct users or callers by directory, the most first, when there are more than the
	// params' ConsumerLimit.
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
