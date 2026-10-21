package kartta

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"github.com/GaijinEntertainment/kartta/bindings/go/wire"
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
	return wire.CancelParams{Build: id}
}

func aRefresh(id uint64) wire.CancelParams {
	return wire.CancelParams{Refresh: id}
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

func openStamped(t *testing.T) *Kartta {
	t.Helper()
	k := openTree(t)
	if _, err := k.Build(t.Context(), wire.BuildOptions{Stamps: true}); err != nil {
		t.Fatal(err)
	}
	return k
}

func Test_Build_cancel(t *testing.T) {
	k := openTree(t)

	t.Run("a cancel that reaches the engine before its build starts stops that build", func(t *testing.T) {
		var answer wire.CancelAnswer
		if err := k.Call("cancel", wire.CancelParams{Build: k.ids.Load() + 1}, &answer); err != nil {
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
	closed := make(chan error, 1)
	go func() { closed <- k.Close() }()
	until(func(k *Kartta) bool { return k.closing }, k)
	k.mu.Unlock()

	if err := <-closed; err != nil {
		t.Fatal(err)
	}
	if err := <-built; !errors.Is(err, ErrClosed) {
		t.Fatalf("Build beside Close = %v, want %v", err, ErrClosed)
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
	closed := make(chan error, 1)
	go func() { closed <- k.Close() }()
	until(func(k *Kartta) bool { return k.closing }, k)
	k.mu.Unlock()

	if err := <-closed; err != nil {
		t.Fatal(err)
	}
	if err := <-refreshed; !errors.Is(err, wire.RefreshCancelled) || !errors.Is(err, ErrClosed) {
		t.Fatalf("Refresh beside Close = %v, want both %s and %v", err, wire.RefreshCancelled, ErrClosed)
	}
}
