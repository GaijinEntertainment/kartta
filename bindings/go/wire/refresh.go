package wire

// RefreshParams are the params of the refresh method.
type RefreshParams struct {
	// ID names the refresh for a cancel, any value 0 included; nil leaves it unnamed. The kartta package passes it on as
	// it is.
	ID *uint64 `json:"id,omitempty"`
	// BindingID names the refresh for a cancel ByBindingID, apart from ID: a binding over the wire names each call it
	// runs by one, so the IDs its callers give never stop a call of its own. The kartta package's Refresh and Call set
	// it, in place of any the params give.
	BindingID uint64 `json:"binding_id,omitempty"`
	// Paths are the root-relative paths that changed, spelled as the index's answers spell them; nil refreshes the
	// whole tree, and an empty list walks nothing. A path that starts with / or \, starts with a drive such as C:, or
	// holds a \ or a .. name is refused with InvalidParams, the message naming which.
	Paths []string `json:"paths"`
}

// Refresh is the answer of the refresh method.
type Refresh struct {
	// Walked counts the parts of the tree walked again.
	Walked int `json:"walked"`
	// Tree reports that the part walked was the whole tree.
	Tree bool `json:"tree"`
	// Added counts the regular files the index holds now and did not hold before.
	Added int `json:"added"`
	// Removed counts the regular files the index held before and does not hold now.
	Removed int `json:"removed"`
	// Extracted counts the files read, new or changed, whose extraction went into a graph.
	Extracted int `json:"extracted"`
	// ResolvedAgain counts the unchanged files read and resolved again, because a question their resolution asked of
	// the tree has another answer now, or their family's configuration changed.
	ResolvedAgain int `json:"resolved_again"`
	// Dropped counts the files the graphs held and hold no more; nil when every graph was built again.
	Dropped *int `json:"dropped,omitempty"`
	// Reconfigured names the language families whose resolver read another configuration than it read before and
	// derived another from it, such as jsts for TypeScript and JavaScript, so that every file of theirs was resolved
	// again.
	Reconfigured []string `json:"reconfigured"`
	// Rebuilt reports that every graph was built again from every file.
	Rebuilt bool `json:"rebuilt"`
	// Widened is set when the paths came to more parts than a refresh walks apart, so it walked the whole tree.
	Widened *Widened `json:"widened,omitempty"`
	// Errors are the failures the refresh collected; Stats lists those the tree fails now.
	Errors BuildErrors `json:"errors"`
	// Watch says whether the index follows its tree.
	Watch Watch `json:"watch"`
}

// Widened says how a refresh of named paths came to walk the whole tree.
type Widened struct {
	// Named counts the paths the refresh was named.
	Named int `json:"named"`
	// Parts counts the parts those paths came to, more than Limit.
	Parts int `json:"parts"`
	// Limit is the most parts the refresh walks apart.
	Limit int `json:"limit"`
}
