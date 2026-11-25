package kartta_test

import (
	"encoding/json"
	"errors"
	"testing"

	"dev.gaijin.team/go/kartta/wire"
)

func Test_ZeroParams(t *testing.T) {
	zero := 0
	noID := uint64(0)
	tests := []struct {
		name     string
		give     any
		wantKeys []string
	}{
		{
			name:     "a walk",
			give:     wire.WalkParams{Line: &zero, Depth: &zero, CandidateLimit: &zero},
			wantKeys: []string{"line", "depth", "candidate_limit"},
		},
		{
			name:     "a call path",
			give:     wire.CallPathParams{FromLine: &zero, ToLine: &zero, Depth: &zero, CandidateLimit: &zero},
			wantKeys: []string{"from_line", "to_line", "depth", "candidate_limit"},
		},
		{
			name:     "references",
			give:     wire.ReferencesParams{Line: &zero, Limit: &zero},
			wantKeys: []string{"line", "limit"},
		},
		{name: "definitions", give: wire.DefinitionsParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{name: "sources", give: wire.SourcesParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{name: "search", give: wire.SearchParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{
			name:     "implementations",
			give:     wire.ImplementationsParams{Line: &zero, Depth: &zero, Limit: &zero},
			wantKeys: []string{"line", "depth", "limit"},
		},
		{
			name:     "dependents",
			give:     wire.DependentsParams{Depth: &zero, MaxFiles: &zero},
			wantKeys: []string{"depth", "max_files"},
		},
		{
			name:     "a segment",
			give:     wire.SegmentParams{Depth: &zero, CallLimit: &zero},
			wantKeys: []string{"depth", "call_limit"},
		},
		{
			name:     "a change",
			give:     wire.ChangeParams{Depth: &zero, ConsumerLimit: &zero, SeedLimit: &zero, HotspotLimit: &zero},
			wantKeys: []string{"depth", "consumer_limit", "seed_limit", "hotspot_limit"},
		},
		{name: "dead code", give: wire.DeadCodeParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{name: "resolution", give: wire.ResolutionParams{Limit: &zero}, wantKeys: []string{"limit"}},
		{name: "grep", give: wire.GrepParams{ID: &noID}, wantKeys: []string{"id"}},
		{name: "a build", give: wire.BuildOptions{ID: &noID, Threads: &zero}, wantKeys: []string{"id", "threads"}},
		{name: "a refresh", give: wire.RefreshParams{ID: &noID}, wantKeys: []string{"id"}},
		{
			name:     "a cancel",
			give:     wire.CancelParams{Build: &noID, Grep: &noID, Refresh: &noID},
			wantKeys: []string{"build", "grep", "refresh"},
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

	depth := 3

	t.Run("a limit of 0 answers the total alone, and nil InvalidParams", func(t *testing.T) {
		counted, err := k.Definitions(wire.DefinitionsParams{Name: "helper", Limit: &zero})
		if err != nil {
			t.Fatal(err)
		}
		_, unbounded := k.Definitions(wire.DefinitionsParams{Name: "helper"})
		if part := counted.Matches[0]; len(part.Found) != 0 || part.Total != 1 {
			t.Errorf("Limit 0 answered %d found of %d, want 0 of 1", len(part.Found), part.Total)
		}
		if !errors.Is(unbounded, wire.InvalidParams) {
			t.Errorf("nil Limit = %v, want %s", unbounded, wire.InvalidParams)
		}
	})

	t.Run("a depth of 0 follows no call, and nil InvalidParams", func(t *testing.T) {
		stayed, err := k.Callers(wire.WalkParams{Name: "helper", Depth: &zero})
		if err != nil {
			t.Fatal(err)
		}
		_, unbounded := k.Callers(wire.WalkParams{Name: "helper"})
		if part := stayed.Matches[0]; len(part.Reached) != 0 || part.Unexpanded != 1 {
			t.Errorf("Depth 0 reached %d, %d unexpanded, want 0, 1", len(part.Reached), part.Unexpanded)
		}
		if !errors.Is(unbounded, wire.InvalidParams) {
			t.Errorf("nil Depth = %v, want %s", unbounded, wire.InvalidParams)
		}
	})

	t.Run("a line of 0 selects nothing, and nil every line", func(t *testing.T) {
		off, err := k.Callers(wire.WalkParams{Name: "helper", Line: &zero, Depth: &depth})
		if err != nil {
			t.Fatal(err)
		}
		every, err := k.Callers(wire.WalkParams{Name: "helper", Depth: &depth})
		if err != nil {
			t.Fatal(err)
		}
		if len(off.Matches) != 0 || off.Unmatched == nil || off.Unmatched.Kind != "off_line" {
			t.Errorf("Line 0 answered %d parts, Unmatched %+v, want none, off_line", len(off.Matches), off.Unmatched)
		}
		if len(every.Matches) != 1 {
			t.Errorf("nil Line answered %d parts, want 1", len(every.Matches))
		}
	})
}

func Test_ZeroThreads(t *testing.T) {
	k := open(t, tree(t))
	zero := 0

	_, err := k.Build(t.Context(), wire.BuildOptions{Threads: &zero})

	if !errors.Is(err, wire.InvalidParams) {
		t.Fatalf("Build with Threads 0 = %v, want %s", err, wire.InvalidParams)
	}
}

func Test_FalseStamps(t *testing.T) {
	k := open(t, tree(t))
	stamps := false

	_, err := k.Build(t.Context(), wire.BuildOptions{Stamps: &stamps, Watch: true})

	if !errors.Is(err, wire.InvalidParams) {
		t.Fatalf("Build with Watch and Stamps false = %v, want %s", err, wire.InvalidParams)
	}
}

func Test_ZeroID(t *testing.T) {
	k := open(t, tree(t))
	id := uint64(0)
	if err := k.Call("cancel", wire.CancelParams{Build: &id}, nil); err != nil {
		t.Fatal(err)
	}

	_, cancelled := k.Build(t.Context(), wire.BuildOptions{ID: &id})
	_, unnamed := k.Build(t.Context(), wire.BuildOptions{})

	if !errors.Is(cancelled, wire.BuildCancelled) {
		t.Errorf("Build under the cancelled ID 0 = %v, want %s", cancelled, wire.BuildCancelled)
	}
	if unnamed != nil {
		t.Errorf("Build with no ID = %v, want a build", unnamed)
	}
}
