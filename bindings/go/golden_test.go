package kartta_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"maps"
	"math"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"dev.gaijin.team/go/kartta"
	"dev.gaijin.team/go/kartta/wire"
)

type replay func(k *kartta.Kartta, params []byte) (any, error)

func typed[P, A any](call func(*kartta.Kartta, P) (*A, error)) replay {
	return func(k *kartta.Kartta, params []byte) (any, error) {
		var decoded P
		decoder := json.NewDecoder(bytes.NewReader(params))
		decoder.DisallowUnknownFields()
		if err := decoder.Decode(&decoded); err != nil {
			return nil, fmt.Errorf("decode the params as %T: %w", decoded, err)
		}
		return call(k, decoded)
	}
}

// constraint: search's scores pass through ln_1p, which Apple's libm and glibc round apart in the last bits
const scoreTolerance = 1e-12

func decode(t *testing.T, text []byte) any {
	t.Helper()
	var value any
	if err := json.Unmarshal(text, &value); err != nil {
		t.Fatalf("decode %s: %v", text, err)
	}
	return value
}

func canonical(t *testing.T, value any) string {
	t.Helper()
	canonical, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	return string(canonical)
}

func pinAgreeingScores(answer, golden any) {
	switch answer := answer.(type) {
	case map[string]any:
		pinned, _ := golden.(map[string]any)
		for key, value := range answer {
			if key == "score" && scoresAgree(value, pinned[key]) {
				answer[key] = pinned[key]
			} else {
				pinAgreeingScores(value, pinned[key])
			}
		}
	case []any:
		pinned, _ := golden.([]any)
		for i := range min(len(answer), len(pinned)) {
			pinAgreeingScores(answer[i], pinned[i])
		}
	}
}

func scoresAgree(answer, pinned any) bool {
	a, aIsNumber := answer.(float64)
	p, pIsNumber := pinned.(float64)
	return aIsNumber && pIsNumber && math.Abs(a-p) <= scoreTolerance*max(math.Abs(a), math.Abs(p))
}

func Test_golden(t *testing.T) {
	// constraint: the skip keys on the repository, not on the golden files, so a checkout that lost them still fails
	if _, err := os.Stat(filepath.Join("..", "..", "Cargo.lock")); errors.Is(err, fs.ErrNotExist) {
		t.Skip("outside a checkout: the golden files live beside the crate, outside the module")
	}
	tests, err := filepath.Abs(filepath.Join("..", "..", "crates", "kartta-ffi", "tests"))
	if err != nil {
		t.Fatal(err)
	}
	paths, err := filepath.Glob(filepath.Join(tests, "golden", "*.json"))
	if err != nil {
		t.Fatal(err)
	}
	build := filepath.Join(tests, "golden", "build.json")
	paths = append([]string{build}, slices.DeleteFunc(paths, func(path string) bool { return path == build })...)
	k := open(t, filepath.Join(tests, "fixture"))

	replays := map[string]replay{
		"build": typed(func(k *kartta.Kartta, options wire.BuildOptions) (*wire.Stats, error) {
			return k.Build(t.Context(), options)
		}),
		"refresh": typed(func(k *kartta.Kartta, params wire.RefreshParams) (*wire.Refresh, error) {
			return k.Refresh(t.Context(), params)
		}),
		"stats":           typed(func(k *kartta.Kartta, _ struct{}) (*wire.Stats, error) { return k.Stats() }),
		"version":         typed(func(k *kartta.Kartta, _ struct{}) (*wire.Version, error) { return k.Version() }),
		"cancel":          typed((*kartta.Kartta).Cancel),
		"resolution":      typed((*kartta.Kartta).Resolution),
		"callers":         typed((*kartta.Kartta).Callers),
		"callees":         typed((*kartta.Kartta).Callees),
		"call_path":       typed((*kartta.Kartta).CallPath),
		"references":      typed((*kartta.Kartta).References),
		"definitions":     typed((*kartta.Kartta).Definitions),
		"sources":         typed((*kartta.Kartta).Sources),
		"search":          typed((*kartta.Kartta).Search),
		"implementations": typed((*kartta.Kartta).Implementations),
		"dead_code":       typed((*kartta.Kartta).DeadCode),
		"outline":         typed((*kartta.Kartta).Outline),
		"dependents":      typed((*kartta.Kartta).Dependents),
		"analyze_segment": typed((*kartta.Kartta).AnalyzeSegment),
		"analyze_change":  typed((*kartta.Kartta).AnalyzeChange),
		"list":            typed((*kartta.Kartta).List),
		"glob":            typed((*kartta.Kartta).Glob),
		"fuzzy":           typed((*kartta.Kartta).Fuzzy),
		"grep": typed(func(k *kartta.Kartta, params wire.GrepParams) (*wire.Grep, error) {
			return k.Grep(t.Context(), params)
		}),
	}
	replayed := make(map[string]bool)
	for _, path := range paths {
		var golden struct {
			Method string          `json:"method"`
			Params json.RawMessage `json:"params"`
			Answer json.RawMessage `json:"answer"`
		}
		text, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if err := json.Unmarshal(text, &golden); err != nil {
			t.Fatalf("%s: %v", path, err)
		}
		replayed[golden.Method] = true

		passed := t.Run(golden.Method, func(t *testing.T) {
			call, ok := replays[golden.Method]
			if !ok {
				t.Fatalf("%s has no typed call", golden.Method)
			}
			answer, err := call(k, golden.Params)

			envelope := map[string]any{"ok": answer}
			var failure *wire.Error
			if errors.As(err, &failure) {
				envelope = map[string]any{"err": failure}
			} else if err != nil {
				t.Fatal(err)
			}
			got, err := json.Marshal(envelope)
			if err != nil {
				t.Fatal(err)
			}
			answered, pinned := decode(t, got), decode(t, golden.Answer)
			pinAgreeingScores(answered, pinned)
			if got, want := canonical(t, answered), canonical(t, pinned); got != want {
				t.Errorf("%s answered\n%s\nwant\n%s", golden.Method, got, want)
			}
		})
		if !passed && golden.Method == "build" {
			t.FailNow()
		}
	}
	for method := range replays {
		if !replayed[method] {
			t.Errorf("no golden file asks %s", method)
		}
	}
	version, err := k.Version()
	if err != nil {
		t.Fatal(err)
	}
	if typed := slices.Sorted(maps.Keys(replays)); !slices.Equal(typed, version.Methods) {
		t.Errorf("the typed calls cover %v, want the methods the library answers, %v", typed, version.Methods)
	}
}

func Test_pinAgreeingScores(t *testing.T) {
	const pinnedScore, pinnedWeight = 3.5799073758139874, 0.7024774156511924
	hits := func(score, weight float64) any {
		return map[string]any{"hits": []any{map[string]any{"score": score, "weight": weight}}}
	}
	tests := []struct {
		name       string
		giveScore  float64
		giveWeight float64
		wantAgree  bool
	}{
		{
			name:       "a score that differs in its last digit agrees with the pinned one",
			giveScore:  3.579907375813988,
			giveWeight: pinnedWeight,
			wantAgree:  true,
		},
		{
			name:       "a score that differs in a real digit does not agree",
			giveScore:  3.579907375913988,
			giveWeight: pinnedWeight,
			wantAgree:  false,
		},
		{
			name:       "a number outside a score must match exactly",
			giveScore:  pinnedScore,
			giveWeight: 0.7024774156511925,
			wantAgree:  false,
		},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			answer, golden := hits(tt.giveScore, tt.giveWeight), hits(pinnedScore, pinnedWeight)
			pinAgreeingScores(answer, golden)
			got, want := canonical(t, answer), canonical(t, golden)
			if agree := got == want; agree != tt.wantAgree {
				t.Errorf("%s against %s: agree %v, want %v", got, want, agree, tt.wantAgree)
			}
		})
	}
}
