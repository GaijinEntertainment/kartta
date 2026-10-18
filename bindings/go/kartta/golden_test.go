package kartta_test

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"github.com/GaijinEntertainment/kartta/bindings/go/kartta"
	"github.com/GaijinEntertainment/kartta/bindings/go/wire"
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

func raw(method string) replay {
	return func(k *kartta.Kartta, params []byte) (any, error) {
		var answer json.RawMessage
		err := k.Call(method, json.RawMessage(params), &answer)
		return answer, err
	}
}

func canonical(t *testing.T, text []byte) string {
	t.Helper()
	var value any
	if err := json.Unmarshal(text, &value); err != nil {
		t.Fatalf("decode %s: %v", text, err)
	}
	canonical, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	return string(canonical)
}

func Test_golden(t *testing.T) {
	tests, err := filepath.Abs(filepath.Join("..", "..", "..", "crates", "kartta-ffi", "tests"))
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
		"stats":           typed(func(k *kartta.Kartta, _ struct{}) (*wire.Stats, error) { return k.Stats() }),
		"version":         typed(func(k *kartta.Kartta, _ struct{}) (*wire.Version, error) { return k.Version() }),
		"callers":         typed((*kartta.Kartta).Callers),
		"call_path":       typed((*kartta.Kartta).CallPath),
		"references":      typed((*kartta.Kartta).References),
		"definitions":     typed((*kartta.Kartta).Definitions),
		"sources":         typed((*kartta.Kartta).Sources),
		"search":          typed((*kartta.Kartta).Search),
		"implementations": typed((*kartta.Kartta).Implementations),
		"outline":         typed((*kartta.Kartta).Outline),
		"dependents":      typed((*kartta.Kartta).Dependents),
		"analyze_segment": typed((*kartta.Kartta).AnalyzeSegment),
		"analyze_change":  typed((*kartta.Kartta).AnalyzeChange),
		"list":            typed((*kartta.Kartta).List),
		"glob":            typed((*kartta.Kartta).Glob),
		"fuzzy":           typed((*kartta.Kartta).Fuzzy),
		"grep":            typed((*kartta.Kartta).Grep),
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
				call = raw(golden.Method)
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
			if got, want := canonical(t, got), canonical(t, golden.Answer); got != want {
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
}
