package kartta_test

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"sync"
	"testing"

	"github.com/GaijinEntertainment/kartta/bindings/go/kartta"
	"github.com/GaijinEntertainment/kartta/bindings/go/wire"
)

func tree(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	files := map[string]string{
		"main.go": "package main\n\nfunc main() {\n\trun()\n}\n",
		"run.go":  "package main\n\nfunc run() {\n\thelper()\n}\n\nfunc helper() {}\n",
	}
	for name, text := range files {
		if err := os.WriteFile(filepath.Join(root, name), []byte(text), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	return root
}

func open(t *testing.T, root string) *kartta.Kartta {
	t.Helper()
	k, err := kartta.Open(root)
	if err != nil {
		t.Fatalf("Open(%q): %v", root, err)
	}
	t.Cleanup(func() { _ = k.Close() })
	return k
}

func Test_Kartta(t *testing.T) {
	t.Run("Open refuses a root that is not a directory", func(t *testing.T) {
		_, err := kartta.Open(filepath.Join(t.TempDir(), "gone"))

		var openError *wire.Error
		if !errors.As(err, &openError) || !errors.Is(err, wire.InvalidRoot) {
			t.Fatalf("Open of a missing root = %v, want a *wire.Error with code %s", err, wire.InvalidRoot)
		}
	})

	t.Run("a query before any build answers NotBuilt", func(t *testing.T) {
		k := open(t, tree(t))

		if _, err := k.Stats(); !errors.Is(err, wire.NotBuilt) {
			t.Fatalf("Stats before Build = %v, want %s", err, wire.NotBuilt)
		}
	})

	t.Run("Version lists the methods", func(t *testing.T) {
		k := open(t, tree(t))

		version, err := k.Version()
		if err != nil {
			t.Fatal(err)
		}
		if version.ABI != 1 || !slices.Contains(version.Methods, "callers") || version.Languages[0] != "go" {
			t.Errorf("Version = %+v", version)
		}
	})

	t.Run("Build then Callers round-trips the wire", func(t *testing.T) {
		k := open(t, tree(t))

		stats, err := k.Build(t.Context(), wire.BuildOptions{Languages: []string{"go"}})
		if err != nil {
			t.Fatal(err)
		}
		walks, err := k.Callers(wire.WalkParams{Name: "helper", Lang: "go"})
		if err != nil {
			t.Fatal(err)
		}

		if stats.Entries != 2 || len(stats.Graphs) != 1 || stats.Graphs[0].Kinds["func"] != 3 {
			t.Errorf("Build = %+v", stats)
		}
		if len(walks.Matches) != 1 {
			t.Fatalf("Callers = %+v, want one match", walks)
		}
		walk := walks.Matches[0]
		var reached []string
		for _, at := range walk.Reached {
			reached = append(reached, at.Symbol.Name)
		}
		if walk.Root.QualifiedName != "main.helper" || !slices.Equal(reached, []string{"run", "main"}) {
			t.Errorf("Callers = %+v, want run then main reaching main.helper", walk)
		}
	})

	t.Run("Build under a done context answers its cause", func(t *testing.T) {
		k := open(t, tree(t))
		ctx, cancel := context.WithCancel(t.Context())
		cancel()

		if _, err := k.Build(ctx, wire.BuildOptions{}); !errors.Is(err, context.Canceled) {
			t.Fatalf("Build = %v, want %v", err, context.Canceled)
		}
	})

	t.Run("a method name holding a NUL byte answers InvalidRequest", func(t *testing.T) {
		k := open(t, tree(t))

		if err := k.Call("version\x00suffix", nil, nil); !errors.Is(err, wire.InvalidRequest) {
			t.Fatalf("Call = %v, want %s", err, wire.InvalidRequest)
		}
	})

	t.Run("an unknown method answers UnknownMethod", func(t *testing.T) {
		k := open(t, tree(t))

		if err := k.Call("engine/callers", nil, nil); !errors.Is(err, wire.UnknownMethod) {
			t.Fatalf("Call = %v, want %s", err, wire.UnknownMethod)
		}
	})

	t.Run("calls after Close answer ErrClosed", func(t *testing.T) {
		k := open(t, tree(t))
		if err := k.Close(); err != nil {
			t.Fatal(err)
		}

		if _, err := k.Stats(); !errors.Is(err, kartta.ErrClosed) {
			t.Fatalf("Stats after Close = %v, want %v", err, kartta.ErrClosed)
		}
		done, cancel := context.WithCancel(t.Context())
		cancel()
		if _, err := k.Build(done, wire.BuildOptions{}); !errors.Is(err, kartta.ErrClosed) {
			t.Fatalf("Build under a done context after Close = %v, want %v", err, kartta.ErrClosed)
		}
		if err := k.Call("stats", make(chan int), nil); !errors.Is(err, kartta.ErrClosed) {
			t.Fatalf("Call with params JSON cannot encode after Close = %v, want %v", err, kartta.ErrClosed)
		}
		if err := k.Close(); err != nil {
			t.Fatalf("second Close = %v", err)
		}
	})

	t.Run("queries from many goroutines share one handle", func(t *testing.T) {
		k := open(t, tree(t))
		if _, err := k.Build(t.Context(), wire.BuildOptions{}); err != nil {
			t.Fatal(err)
		}

		var wg sync.WaitGroup
		failures := make(chan error, 32)
		for range 16 {
			wg.Go(func() {
				if _, err := k.Callers(wire.WalkParams{Name: "helper"}); err != nil {
					failures <- err
				}
				if _, err := k.Stats(); err != nil {
					failures <- err
				}
			})
		}
		wg.Wait()
		close(failures)

		for err := range failures {
			t.Error(err)
		}
	})
}
