package wire

// WalkParams are the params of the callers method.
type WalkParams struct {
	// Name selects the symbol to start from: a bare or a qualified name.
	Name string `json:"name"`
	// Lang names the graph to ask, by a language or its family; empty asks every graph.
	Lang string `json:"lang,omitempty"`
	// Depth is how many calls away the walk goes; 0 takes the default, 3.
	Depth int `json:"depth,omitempty"`
	// MinConfidence is the least confidence, 0 to 100, a call needs to be taken.
	MinConfidence int `json:"min_confidence,omitempty"`
	// MaxSymbols caps the symbols the walk reaches, its root included; 0 takes the default, 500.
	MaxSymbols int `json:"max_symbols,omitempty"`
}

// Walks is the answer of a walk over calls: one Walk for each graph the name selects a symbol in.
type Walks struct {
	Matches []Walk `json:"matches"`
}

// Walk is what a walk over calls reached in one graph, and what it left out.
type Walk struct {
	Graph string     `json:"graph"`
	Root  Definition `json:"root"`
	// PassedOver counts the other symbols the name selects, of which Root was chosen.
	PassedOver int       `json:"passed_over"`
	Reached    []Reached `json:"reached"`
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
