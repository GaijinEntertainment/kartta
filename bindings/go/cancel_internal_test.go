package kartta

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"testing"

	"dev.gaijin.team/go/kartta/wire"
)

func openTree(t *testing.T) *Kartta {
	t.Helper()
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "main.go"), []byte("package main\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	k, err := Open(root)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = k.Close() })
	return k
}

func cancelAhead(t *testing.T, k *Kartta, naming func(id uint64) wire.CancelParams) uint64 {
	t.Helper()
	id := k.ids.Load() + 1
	if err := k.Call("cancel", naming(id), nil); err != nil {
		t.Fatal(err)
	}
	return id
}

func aBuild(id uint64) wire.CancelParams {
	return wire.CancelParams{Build: &id, By: wire.ByBindingID}
}

func aRefresh(id uint64) wire.CancelParams {
	return wire.CancelParams{Refresh: &id, By: wire.ByBindingID}
}

func until(holds func(k *Kartta) bool, k *Kartta) {
	for {
		k.tracked.Lock()
		held := holds(k)
		k.tracked.Unlock()
		if held {
			return
		}
		runtime.Gosched()
	}
}

func tracking(k *Kartta, id uint64, run func() error) <-chan error {
	done := make(chan error, 1)
	go func() { done <- run() }()
	until(func(k *Kartta) bool { _, tracked := k.stopping[id]; return tracked }, k)
	return done
}

func closeHolding(t *testing.T, k *Kartta) {
	t.Helper()
	// constraint: a tracked call has yet to reach k.mu, and one that reaches it after Close answers ErrClosed alone.
	stacks := make([]byte, 1<<20)
	for !bytes.Contains(stacks[:runtime.Stack(stacks, true)], []byte("[sync.RWMutex.RLock")) {
		runtime.Gosched()
	}
	closed := make(chan error, 1)
	go func() { closed <- k.Close() }()
	until(func(k *Kartta) bool { return k.closing }, k)
	k.mu.Unlock()
	if err := <-closed; err != nil {
		t.Fatal(err)
	}
}

func buildTracked(ctx context.Context, k *Kartta, id uint64) <-chan error {
	return tracking(k, id, func() error {
		_, err := k.Build(ctx, wire.BuildOptions{})
		return err
	})
}

func refreshTracked(ctx context.Context, k *Kartta, id uint64) <-chan error {
	return tracking(k, id, func() error {
		_, err := k.Refresh(ctx, wire.RefreshParams{})
		return err
	})
}

func grepTracked(ctx context.Context, k *Kartta, id uint64) (<-chan error, *wire.Grep) {
	answer := new(wire.Grep)
	return tracking(k, id, func() error {
		grep, err := k.Grep(ctx, wire.GrepParams{Patterns: []string{"package"}})
		if err == nil {
			*answer = *grep
		}
		return err
	}), answer
}

func openStamped(t *testing.T) *Kartta {
	t.Helper()
	k := openTree(t)
	stamps := true
	if _, err := k.Build(t.Context(), wire.BuildOptions{Stamps: &stamps}); err != nil {
		t.Fatal(err)
	}
	return k
}

func Test_Build_cancel(t *testing.T) {
	k := openTree(t)

	t.Run("a cancel that reaches the engine before its build starts stops that build", func(t *testing.T) {
		var answer wire.CancelAnswer
		if err := k.Call("cancel", aBuild(k.ids.Load()+1), &answer); err != nil {
			t.Fatal(err)
		}

		_, err := k.Build(context.Background(), wire.BuildOptions{})

		if answer.Stopped || !errors.Is(err, wire.BuildCancelled) {
			t.Fatalf("cancel = %+v, Build = %v, want a build that starts cancelled", answer, err)
		}
		if _, err := k.Stats(); !errors.Is(err, wire.NotBuilt) {
			t.Fatalf("Stats after a cancelled build = %v, want %s", err, wire.NotBuilt)
		}
	})

	t.Run("a context done once its build started answers both BuildCancelled and its cause", func(t *testing.T) {
		id := cancelAhead(t, k, aBuild)
		ctx, cancel := context.WithCancel(t.Context())
		k.mu.Lock()
		built := buildTracked(ctx, k, id)
		cancel()
		k.mu.Unlock()

		err := <-built

		if !errors.Is(err, wire.BuildCancelled) || !errors.Is(err, context.Canceled) {
			t.Fatalf("Build = %v, want both %s and %v", err, wire.BuildCancelled, context.Canceled)
		}
	})

	t.Run("a cancel naming the id a build through Call gives stops that build", func(t *testing.T) {
		id := uint64(77)
		if err := k.Call("cancel", wire.CancelParams{Build: &id}, nil); err != nil {
			t.Fatal(err)
		}

		if err := k.Call("build", map[string]uint64{"id": id}, nil); !errors.Is(err, wire.BuildCancelled) {
			t.Fatalf("Call(build) under a cancelled id = %v, want %s", err, wire.BuildCancelled)
		}
	})

	t.Run("a cancel naming the ID a Build's options give stops that build", func(t *testing.T) {
		id := uint64(78)
		if err := k.Call("cancel", wire.CancelParams{Build: &id}, nil); err != nil {
			t.Fatal(err)
		}

		if _, err := k.Build(t.Context(), wire.BuildOptions{ID: &id}); !errors.Is(err, wire.BuildCancelled) {
			t.Fatalf("Build under a cancelled ID = %v, want %s", err, wire.BuildCancelled)
		}
	})

	t.Run("the next build runs", func(t *testing.T) {
		if _, err := k.Build(t.Context(), wire.BuildOptions{}); err != nil {
			t.Fatal(err)
		}
	})
}

func Test_Close_stops_a_build_in_flight(t *testing.T) {
	k := openTree(t)
	id := cancelAhead(t, k, aBuild)
	k.mu.Lock()
	built := buildTracked(t.Context(), k, id)
	closeHolding(t, k)

	if err := <-built; !errors.Is(err, ErrClosed) {
		t.Fatalf("Build beside Close = %v, want %v", err, ErrClosed)
	}
}

func Test_Grep_cancel(t *testing.T) {
	k := openTree(t)
	if _, err := k.Build(t.Context(), wire.BuildOptions{}); err != nil {
		t.Fatal(err)
	}

	t.Run("a context done before its grep enters stops it before it opens a file", func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		k.mu.Lock()
		grepped, answer := grepTracked(ctx, k, k.ids.Load()+1)
		cancel()
		k.mu.Unlock()

		if err := <-grepped; err != nil {
			t.Fatal(err)
		}
		if !answer.Cancelled || answer.Searched != 0 || answer.Unsearched != answer.Candidates {
			t.Errorf("Grep = %+v, want one cancelled with every candidate unsearched", answer)
		}
	})

	t.Run("the next grep answers whole", func(t *testing.T) {
		answer, err := k.Grep(t.Context(), wire.GrepParams{Patterns: []string{"package"}})
		if err != nil {
			t.Fatal(err)
		}
		if answer.Cancelled || answer.Searched != answer.Candidates || len(answer.Files) != 1 {
			t.Errorf("Grep = %+v, want every candidate searched and one file", answer)
		}
	})
}

func Test_Close_stops_a_grep_yet_to_enter(t *testing.T) {
	k := openTree(t)
	if _, err := k.Build(t.Context(), wire.BuildOptions{}); err != nil {
		t.Fatal(err)
	}
	k.mu.Lock()
	grepped, answer := grepTracked(t.Context(), k, k.ids.Load()+1)
	closeHolding(t, k)

	if err := <-grepped; err != nil {
		t.Fatal(err)
	}
	if !answer.Cancelled || answer.Searched != 0 || answer.Unsearched != answer.Candidates {
		t.Errorf("Grep beside Close = %+v, want one cancelled with every candidate unsearched", answer)
	}
}

func Test_Refresh_cancel(t *testing.T) {
	k := openStamped(t)

	t.Run("a context done once its refresh started answers both RefreshCancelled and its cause", func(t *testing.T) {
		id := cancelAhead(t, k, aRefresh)
		ctx, cancel := context.WithCancel(t.Context())
		k.mu.Lock()
		refreshed := refreshTracked(ctx, k, id)
		cancel()
		k.mu.Unlock()

		err := <-refreshed

		if !errors.Is(err, wire.RefreshCancelled) || !errors.Is(err, context.Canceled) {
			t.Fatalf("Refresh = %v, want both %s and %v", err, wire.RefreshCancelled, context.Canceled)
		}
	})

	t.Run("the index answers as it did, and the next refresh runs", func(t *testing.T) {
		if _, err := k.Stats(); err != nil {
			t.Fatal(err)
		}
		if _, err := k.Refresh(t.Context(), wire.RefreshParams{}); err != nil {
			t.Fatal(err)
		}
	})
}

func Test_Close_stops_a_refresh_in_flight(t *testing.T) {
	k := openStamped(t)
	id := cancelAhead(t, k, aRefresh)
	k.mu.Lock()
	refreshed := refreshTracked(t.Context(), k, id)
	closeHolding(t, k)

	if err := <-refreshed; !errors.Is(err, wire.RefreshCancelled) || !errors.Is(err, ErrClosed) {
		t.Fatalf("Refresh beside Close = %v, want both %s and %v", err, wire.RefreshCancelled, ErrClosed)
	}
}

func openFiles(t *testing.T, files int) *Kartta {
	t.Helper()
	root := t.TempDir()
	for file := range files {
		source := fmt.Sprintf("package main\n\nfunc f%d() {\n\tf%d()\n}\n", file, (file+1)%files)
		if err := os.WriteFile(filepath.Join(root, fmt.Sprintf("f%d.go", file)), []byte(source), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	k, err := Open(root)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = k.Close() })
	return k
}

func walking(ctx context.Context, t *testing.T, k *Kartta) <-chan error {
	t.Helper()
	built := make(chan error, 1)
	go func() {
		_, err := k.Build(ctx, wire.BuildOptions{})
		built <- err
	}()
	_, err := k.Stats()
	for errors.Is(err, wire.NotBuilt) {
		_, err = k.Stats()
	}
	if !errors.Is(err, wire.Building) {
		t.Fatalf("Stats = %v before a query saw the build walking; give it a larger tree", err)
	}
	return built
}

func Test_Call_ids(t *testing.T) {
	t.Run("a cancel by a raw call's id stops it and not a build of the handle's in flight", func(t *testing.T) {
		k := openFiles(t, 3000)
		id := k.ids.Load() + 1
		typed := walking(t.Context(), t, k)
		raw := make(chan error, 1)
		go func() { raw <- k.Call("build", map[string]uint64{"id": id}, nil) }()

		if err := k.Call("cancel", wire.CancelParams{Build: &id}, nil); err != nil {
			t.Fatal(err)
		}

		if err := <-typed; err != nil {
			t.Errorf("Build beside a cancel naming a raw call's id = %v, want its stats", err)
		}
		if err := <-raw; !errors.Is(err, wire.BuildCancelled) {
			t.Errorf("Call(build) under the cancelled id = %v, want %s", err, wire.BuildCancelled)
		}
	})

	t.Run("a context done stops its build and not a raw call given its binding ID as an id", func(t *testing.T) {
		k := openFiles(t, 3000)
		id := k.ids.Load() + 1
		ctx, cancel := context.WithCancel(t.Context())
		typed := walking(ctx, t, k)
		raw := make(chan error, 1)
		go func() { raw <- k.Call("build", map[string]uint64{"id": id}, nil) }()

		cancel()

		if err := <-typed; !errors.Is(err, wire.BuildCancelled) || !errors.Is(err, context.Canceled) {
			t.Errorf("Build under a context done = %v, want both %s and %v", err, wire.BuildCancelled, context.Canceled)
		}
		if err := <-raw; err != nil {
			t.Errorf("Call(build) beside it = %v, want its stats", err)
		}
	})

	t.Run("a cancel by a finished raw call's id leaves the handle's next build running", func(t *testing.T) {
		k := openTree(t)
		id := k.ids.Load() + 2
		if err := k.Call("build", map[string]uint64{"id": id}, nil); err != nil {
			t.Fatal(err)
		}
		if err := k.Call("cancel", wire.CancelParams{Build: &id}, nil); err != nil {
			t.Fatal(err)
		}

		if _, err := k.Build(t.Context(), wire.BuildOptions{}); err != nil {
			t.Fatalf("Build after a cancel naming a finished raw call's id = %v, want its stats", err)
		}
	})
}

func Test_withBindingID(t *testing.T) {
	t.Run("the id the params give passes on beside the binding ID, which replaces theirs", func(t *testing.T) {
		body, ok := withBindingID(map[string]any{"id": 77, "binding_id": 5, "patterns": []string{"package"}}, 9)

		var got struct {
			ID        uint64   `json:"id"`
			BindingID uint64   `json:"binding_id"`
			Patterns  []string `json:"patterns"`
		}
		if err := json.Unmarshal(body, &got); err != nil {
			t.Fatal(err)
		}
		if !ok || got.ID != 77 || got.BindingID != 9 || !slices.Equal(got.Patterns, []string{"package"}) {
			t.Errorf("withBindingID = %s, %v, want the patterns under id 77 and binding ID 9", body, ok)
		}
	})

	t.Run("params that are no object are not named", func(t *testing.T) {
		for _, give := range []any{[]int{1}, "grep"} {
			if body, ok := withBindingID(give, 9); ok {
				t.Errorf("withBindingID(%v) = %s, want false", give, body)
			}
		}
	})
}

func closeBeside(t *testing.T, k *Kartta, run func() error) error {
	t.Helper()
	k.mu.Lock()
	ran := tracking(k, k.ids.Load()+1, run)
	closeHolding(t, k)
	return <-ran
}

func Test_Close_stops_a_raw_call_yet_to_enter(t *testing.T) {
	grep := func(t *testing.T, params any) {
		k := openTree(t)
		if _, err := k.Build(t.Context(), wire.BuildOptions{}); err != nil {
			t.Fatal(err)
		}
		var answer wire.Grep

		if err := closeBeside(t, k, func() error { return k.Call("grep", params, &answer) }); err != nil {
			t.Fatal(err)
		}

		if !answer.Cancelled || answer.Searched != 0 || answer.Unsearched != answer.Candidates {
			t.Errorf("Call(grep) beside Close = %+v, want one cancelled with every candidate unsearched", answer)
		}
	}

	t.Run("a grep under an id of the handle's", func(t *testing.T) {
		grep(t, wire.GrepParams{Patterns: []string{"package"}})
	})

	t.Run("a grep under its own id", func(t *testing.T) {
		grep(t, map[string]any{"id": 77, "patterns": []string{"package"}})
	})

	t.Run("a build", func(t *testing.T) {
		k := openTree(t)

		err := closeBeside(t, k, func() error { return k.Call("build", nil, nil) })

		if !errors.Is(err, wire.BuildCancelled) || !errors.Is(err, ErrClosed) {
			t.Fatalf("Call(build) beside Close = %v, want both %s and %v", err, wire.BuildCancelled, ErrClosed)
		}
	})

	t.Run("a refresh", func(t *testing.T) {
		k := openStamped(t)

		err := closeBeside(t, k, func() error { return k.Call("refresh", json.RawMessage(`{}`), nil) })

		if !errors.Is(err, wire.RefreshCancelled) || !errors.Is(err, ErrClosed) {
			t.Fatalf("Call(refresh) beside Close = %v, want both %s and %v", err, wire.RefreshCancelled, ErrClosed)
		}
	})
}
