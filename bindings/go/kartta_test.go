package kartta_test

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"dev.gaijin.team/go/kartta"
	"dev.gaijin.team/go/kartta/wire"
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

// eventually reports whether done answers true before 20 s pass, asking it every 20 ms.
func eventually(done func() bool) bool {
	deadline := time.After(20 * time.Second)
	tick := time.NewTicker(20 * time.Millisecond)
	defer tick.Stop()
	for !done() {
		select {
		case <-deadline:
			return done()
		case <-tick.C:
		}
	}
	return true
}

// watchThreads counts the threads of this process named kartta-watch, the name the library gives a watch's thread.
func watchThreads(t *testing.T) int {
	t.Helper()
	tasks, err := os.ReadDir("/proc/self/task")
	if err != nil {
		t.Fatal(err)
	}
	watching := 0
	for _, task := range tasks {
		name, err := os.ReadFile(filepath.Join("/proc/self/task", task.Name(), "comm"))
		if err == nil && strings.TrimSpace(string(name)) == "kartta-watch" {
			watching++
		}
	}
	return watching
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

	t.Run("Build keeps the graphs of a content index it cannot store", func(t *testing.T) {
		root := tree(t)
		k := open(t, root)

		stats, err := k.Build(t.Context(), wire.BuildOptions{
			Languages: []string{"go"},
			ContentIndex: &wire.ContentIndex{
				Storage: wire.StorageFile,
				Dir:     filepath.Join(root, "gone"),
			},
		})
		if err != nil {
			t.Fatal(err)
		}

		if stats.ContentIndex != nil || !strings.Contains(stats.ContentIndexFailure, "gone") {
			t.Errorf("Build = %+v, want no content index and why", stats)
		}
		if len(stats.Graphs) != 1 || len(stats.Errors.Listed) != 1 || stats.Errors.Listed[0].Kind != "content_index" {
			t.Errorf("Build = %+v, want the graph and the failure", stats)
		}
	})

	t.Run("Build then Callers round-trips the wire", func(t *testing.T) {
		k := open(t, tree(t))

		stats, err := k.Build(t.Context(), wire.BuildOptions{Languages: []string{"go"}})
		if err != nil {
			t.Fatal(err)
		}
		depth := 3
		walks, err := k.Callers(wire.WalkParams{Name: "helper", Lang: "go", Depth: &depth})
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

	t.Run("a watched build takes an edit in without a Refresh", func(t *testing.T) {
		root := tree(t)
		k := open(t, root)

		stats, err := k.Build(t.Context(), wire.BuildOptions{Languages: []string{"go"}, Watch: true})
		if err != nil {
			t.Fatal(err)
		}
		edited := "package main\n\nfunc run() {\n\thelper()\n}\n\nfunc helper() {}\n\nfunc extra() {}\n"
		if err := os.WriteFile(filepath.Join(root, "run.go"), []byte(edited), 0o600); err != nil {
			t.Fatal(err)
		}

		if !stats.Watch.Following {
			t.Fatalf("Build.Watch = %+v, want following", stats.Watch)
		}
		limit := 1
		defined := func() bool {
			found, err := k.Definitions(wire.DefinitionsParams{Name: "extra", Limit: &limit})
			return err == nil && len(found.Matches) == 1 && len(found.Matches[0].Found) == 1
		}
		if !eventually(defined) {
			t.Fatal("the watch never took the edit in")
		}
	})

	t.Run("Close ends the watch's thread", func(t *testing.T) {
		if runtime.GOOS != "linux" {
			t.Skip("only Linux lists a process's thread names, under /proc")
		}
		k := open(t, tree(t))
		if _, err := k.Build(t.Context(), wire.BuildOptions{Watch: true}); err != nil {
			t.Fatal(err)
		}
		// constraint: Build answers once the watch thread exists, and std names a thread from inside it, so the name can lag
		watching := eventually(func() bool { return watchThreads(t) > 0 })

		if err := k.Close(); err != nil {
			t.Fatal(err)
		}

		if !watching {
			t.Fatal("no kartta-watch thread while the build watched")
		}
		if !eventually(func() bool { return watchThreads(t) == 0 }) {
			t.Fatalf("%d kartta-watch threads outlived Close", watchThreads(t))
		}
	})

	t.Run("a pattern nested to the parser's limit survives a thread's default stack", func(t *testing.T) {
		k := open(t, tree(t))
		if _, err := k.Build(t.Context(), wire.BuildOptions{Languages: []string{}}); err != nil {
			t.Fatal(err)
		}
		pattern := strings.Repeat("(a|", 124) + "func" + strings.Repeat(")", 124)

		// Locked at once, the goroutines hold as many threads, and only one can be the main thread, whose stack is
		// the process's and not the default a cgo thread takes.
		const threads = 4
		var locked, calls sync.WaitGroup
		locked.Add(threads)
		for range threads {
			calls.Go(func() {
				runtime.LockOSThread()
				defer runtime.UnlockOSThread()
				locked.Done()
				locked.Wait()
				answer, err := k.Grep(t.Context(), wire.GrepParams{Patterns: []string{pattern}})
				if err != nil {
					t.Error(err)
					return
				}
				if answer.Lines == 0 {
					t.Error("Grep found no line holding func")
				}
			})
		}
		calls.Wait()
	})

	t.Run("queries from many goroutines share one handle", func(t *testing.T) {
		k := open(t, tree(t))
		if _, err := k.Build(t.Context(), wire.BuildOptions{}); err != nil {
			t.Fatal(err)
		}

		var wg sync.WaitGroup
		failures := make(chan error, 32)
		depth := 3
		for range 16 {
			wg.Go(func() {
				if _, err := k.Callers(wire.WalkParams{Name: "helper", Depth: &depth}); err != nil {
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
