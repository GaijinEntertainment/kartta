package kartta

/*
#cgo CFLAGS: -I${SRCDIR}/../../../crates/kartta-ffi/include
#cgo LDFLAGS: -lkartta_ffi -lm
#cgo darwin LDFLAGS: -liconv
#cgo linux LDFLAGS: -ldl -lpthread
#include <stdlib.h>
#include "kartta.h"
*/
import "C"

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"unsafe"

	"github.com/GaijinEntertainment/kartta/bindings/go/wire"
)

// ErrClosed is returned by every method of a Kartta after Close.
var ErrClosed = errors.New("kartta: the handle is closed")

// ErrABI is returned by Open when the linked library implements another ABI version than the header the package was
// compiled against: the archive and the binding come from different kartta commits.
var ErrABI = errors.New("kartta: the library's ABI version differs from the header's")

// Kartta is a handle on one tree.
type Kartta struct {
	mu      sync.RWMutex
	handle  *C.KarttaHandle
	cleanup runtime.Cleanup
	builds  atomic.Uint64

	tracked  sync.Mutex
	closing  bool
	stopping map[uint64]context.CancelCauseFunc
}

// Open opens a handle on the tree at root. Nothing is walked until Build. A failure is a *wire.Error, or ErrABI.
func Open(root string) (*Kartta, error) {
	if got := C.kartta_abi_version(); got != C.KARTTA_ABI_VERSION {
		return nil, fmt.Errorf("%w: the library implements %d, the header %d", ErrABI, got, C.KARTTA_ABI_VERSION)
	}
	params, err := json.Marshal(struct {
		Root string `json:"root"`
	}{root})
	if err != nil {
		return nil, fmt.Errorf("kartta: encode the open params: %w", err)
	}
	var failure C.KarttaString
	handle := C.kartta_open(bytesOf(params), C.size_t(len(params)), &failure)
	if handle == nil {
		defer C.kartta_string_free(failure)
		var openError wire.Error
		if err := json.Unmarshal(bytesAt(failure), &openError); err != nil {
			return nil, fmt.Errorf("kartta: decode the open error: %w", err)
		}
		return nil, &openError
	}
	k := &Kartta{handle: handle, stopping: make(map[uint64]context.CancelCauseFunc)}
	k.cleanup = runtime.AddCleanup(k, func(handle *C.KarttaHandle) { C.kartta_free(handle) }, handle)
	return k, nil
}

// Close cancels the builds in flight, waits for the calls in flight to return, and frees the handle. A build Close
// cancels answers an error matching both wire.BuildCancelled and ErrClosed. Closing a closed handle does nothing.
func (k *Kartta) Close() error {
	k.tracked.Lock()
	k.closing = true
	for _, stop := range k.stopping {
		stop(ErrClosed)
	}
	k.tracked.Unlock()
	_ = k.Call("cancel", wire.CancelParams{}, nil)
	k.mu.Lock()
	defer k.mu.Unlock()
	if k.handle == nil {
		return nil
	}
	k.cleanup.Stop()
	C.kartta_free(k.handle)
	k.handle = nil
	return nil
}

// Build walks the tree and builds the file index and the graphs options name, replacing what an earlier build made.
// Once ctx is done, a build still running stops and answers an error matching both wire.BuildCancelled and ctx's
// cause; one that finished first answers its stats, and the handle holds what it built. A build that fails or stops
// leaves the handle without an index until the next one finishes. A ctx done before the call answers its cause alone
// and leaves the handle as it was.
func (k *Kartta) Build(ctx context.Context, options wire.BuildOptions) (*wire.Stats, error) {
	if err := context.Cause(ctx); err != nil {
		if k.closed() {
			return nil, ErrClosed
		}
		return nil, err
	}
	options.ID = k.builds.Add(1)
	ctx, stopBuild := context.WithCancelCause(ctx)
	defer stopBuild(nil)
	if !k.track(options.ID, stopBuild) {
		return nil, ErrClosed
	}
	defer k.untrack(options.ID)

	k.mu.RLock()
	defer k.mu.RUnlock()
	if k.handle == nil {
		return nil, ErrClosed
	}
	handle := k.handle
	cancelSent := make(chan struct{})
	stop := context.AfterFunc(ctx, func() {
		defer close(cancelSent)
		_ = call(handle, "cancel", wire.CancelParams{Build: options.ID}, nil)
	})
	defer func() {
		if !stop() {
			<-cancelSent
		}
	}()
	var stats wire.Stats
	if err := call(handle, "build", options, &stats); err != nil {
		if cause := context.Cause(ctx); cause != nil && errors.Is(err, wire.BuildCancelled) {
			return nil, fmt.Errorf("%w: %w", err, cause)
		}
		return nil, err
	}
	return &stats, nil
}

func (k *Kartta) track(id uint64, stop context.CancelCauseFunc) bool {
	k.tracked.Lock()
	defer k.tracked.Unlock()
	if k.closing {
		return false
	}
	k.stopping[id] = stop
	return true
}

func (k *Kartta) closed() bool {
	k.tracked.Lock()
	defer k.tracked.Unlock()
	return k.closing
}

func (k *Kartta) untrack(id uint64) {
	k.tracked.Lock()
	defer k.tracked.Unlock()
	delete(k.stopping, id)
}

// Stats answers what the last build indexed, what it made of each language's files, and what each graph holds.
func (k *Kartta) Stats() (*wire.Stats, error) {
	var stats wire.Stats
	if err := k.Call("stats", nil, &stats); err != nil {
		return nil, err
	}
	return &stats, nil
}

// Callers answers the callers of the symbol params name, transitively, in each graph that holds one.
func (k *Kartta) Callers(params wire.WalkParams) (*wire.Walks, error) {
	var walks wire.Walks
	if err := k.Call("callers", params, &walks); err != nil {
		return nil, err
	}
	return &walks, nil
}

// Version answers the library's ABI version, its crate version, the methods it answers and the languages it ships.
func (k *Kartta) Version() (*wire.Version, error) {
	var version wire.Version
	if err := k.Call("version", nil, &version); err != nil {
		return nil, err
	}
	return &version, nil
}

// Call answers method with params, encoded as JSON, decoding the answer into answer, a pointer, or discarding it when
// answer is nil. A failure the engine answers is a *wire.Error. Call reaches methods this package has no wrapper for.
func (k *Kartta) Call(method string, params, answer any) error {
	k.mu.RLock()
	defer k.mu.RUnlock()
	if k.handle == nil {
		return ErrClosed
	}
	return call(k.handle, method, params, answer)
}

func call(handle *C.KarttaHandle, method string, params, answer any) error {
	if strings.ContainsRune(method, 0) {
		return &wire.Error{Code: wire.InvalidRequest, Message: fmt.Sprintf("the method %q holds a NUL byte", method)}
	}
	body, err := json.Marshal(params)
	if err != nil {
		return fmt.Errorf("kartta: encode the %s params: %w", method, err)
	}
	name := C.CString(method)
	defer C.free(unsafe.Pointer(name))
	answered := C.kartta_call(handle, name, bytesOf(body), C.size_t(len(body)))
	defer C.kartta_string_free(answered)

	envelope := struct {
		Ok  any         `json:"ok"`
		Err *wire.Error `json:"err"`
	}{Ok: answer}
	if answer == nil {
		envelope.Ok = new(json.RawMessage)
	}
	if err := json.Unmarshal(bytesAt(answered), &envelope); err != nil {
		return fmt.Errorf("kartta: decode the %s answer: %w", method, err)
	}
	if envelope.Err != nil {
		return envelope.Err
	}
	return nil
}

func bytesOf(b []byte) *C.uint8_t {
	return (*C.uint8_t)(unsafe.Pointer(unsafe.SliceData(b)))
}

func bytesAt(s C.KarttaString) []byte {
	return unsafe.Slice((*byte)(unsafe.Pointer(s.ptr)), int(s.len))
}
