package kartta_test

import (
	"encoding/json"
	"testing"

	"dev.gaijin.team/go/kartta/wire"
)

func Test_ZeroParams(t *testing.T) {
	zero := 0
	noBytes := uint64(0)
	tests := []struct {
		name     string
		give     any
		wantKeys []string
	}{
		{
			name:     "a walk",
			give:     wire.WalkParams{Depth: &zero, MaxSymbols: &zero},
			wantKeys: []string{"depth", "max_symbols"},
		},
		{
			name:     "a call path",
			give:     wire.CallPathParams{Depth: &zero, MaxSymbols: &zero},
			wantKeys: []string{"depth", "max_symbols"},
		},
		{name: "references", give: wire.ReferencesParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{name: "definitions", give: wire.DefinitionsParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{name: "sources", give: wire.SourcesParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{name: "search", give: wire.SearchParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{
			name:     "implementations",
			give:     wire.ImplementationsParams{Depth: &zero, Limit: &zero},
			wantKeys: []string{"depth", "limit"},
		},
		{name: "dependents", give: wire.DependentsParams{Depth: &zero}, wantKeys: []string{"depth"}},
		{
			name:     "a segment",
			give:     wire.SegmentParams{Depth: &zero, MaxSymbols: &zero},
			wantKeys: []string{"depth", "max_symbols"},
		},
		{
			name:     "a change",
			give:     wire.ChangeParams{Depth: &zero, MaxSymbols: &zero},
			wantKeys: []string{"depth", "max_symbols"},
		},
		{name: "fuzzy", give: wire.FuzzyParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{
			name:     "grep",
			give:     wire.GrepParams{MaxLineBytes: &zero, MaxFileBytes: &noBytes},
			wantKeys: []string{"max_line_bytes", "max_file_bytes"},
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			body, err := json.Marshal(tt.give)
			if err != nil {
				t.Fatal(err)
			}
			var sent map[string]any
			if err := json.Unmarshal(body, &sent); err != nil {
				t.Fatal(err)
			}
			for _, key := range tt.wantKeys {
				if sent[key] != float64(0) {
					t.Errorf("%s sent as %v in %s, want 0", key, sent[key], body)
				}
			}
		})
	}
}

func Test_ZeroBound(t *testing.T) {
	k := open(t, tree(t))
	if _, err := k.Build(t.Context(), wire.BuildOptions{}); err != nil {
		t.Fatal(err)
	}
	zero := 0

	t.Run("a limit of 0 answers the total alone, and nil the default", func(t *testing.T) {
		counted, err := k.Definitions(wire.DefinitionsParams{Name: "helper", Limit: &zero})
		if err != nil {
			t.Fatal(err)
		}
		listed, err := k.Definitions(wire.DefinitionsParams{Name: "helper"})
		if err != nil {
			t.Fatal(err)
		}
		if part := counted.Matches[0]; len(part.Found) != 0 || part.Total != 1 {
			t.Errorf("Limit 0 answered %d found of %d, want 0 of 1", len(part.Found), part.Total)
		}
		if part := listed.Matches[0]; len(part.Found) != 1 {
			t.Errorf("nil Limit answered %d found, want 1", len(part.Found))
		}
	})

	t.Run("a depth of 0 follows no call, and nil the default", func(t *testing.T) {
		stayed, err := k.Callers(wire.WalkParams{Name: "helper", Depth: &zero})
		if err != nil {
			t.Fatal(err)
		}
		walked, err := k.Callers(wire.WalkParams{Name: "helper"})
		if err != nil {
			t.Fatal(err)
		}
		if part := stayed.Matches[0]; len(part.Reached) != 0 || part.Unexpanded != 1 {
			t.Errorf("Depth 0 reached %d, %d unexpanded, want 0, 1", len(part.Reached), part.Unexpanded)
		}
		if part := walked.Matches[0]; len(part.Reached) != 2 {
			t.Errorf("nil Depth reached %d, want 2", len(part.Reached))
		}
	})
}
