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
	ids     atomic.Uint64

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

// Close cancels the builds, greps and refreshes in flight, waits for the calls in flight to return, and frees the
// handle. A build Close cancels answers an error matching both wire.BuildCancelled and ErrClosed, and a refresh one
// matching both wire.RefreshCancelled and ErrClosed; of the builds and the refreshes that have yet to enter the
// library, only the last 1,024 of each it cancels stop. Closing a closed handle does nothing.
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
	return tracked[wire.Stats](ctx, k, "build", wire.BuildCancelled, func(id uint64) (any, wire.CancelParams) {
		options.ID = id
		return options, wire.CancelParams{Build: id}
	})
}

// Refresh brings the index and the graphs up to the tree in place, walking again the paths params name, or the whole
// tree when Paths is nil, and answers what changed. Every call waits for a refresh under way. A handle built without
// Stamps answers wire.NoStamps. Once ctx is done, a refresh still walking stops and answers an error matching both
// wire.RefreshCancelled and ctx's cause, leaving the index as it was; one past its walk finishes. A ctx done before the
// call answers its cause alone.
func (k *Kartta) Refresh(ctx context.Context, params wire.RefreshParams) (*wire.Refresh, error) {
	return tracked[wire.Refresh](ctx, k, "refresh", wire.RefreshCancelled, func(id uint64) (any, wire.CancelParams) {
		params.ID = id
		return params, wire.CancelParams{Refresh: id}
	})
}

// tracked calls method under an id of its own, which named gives the params it sends and the cancel that stops it.
// Close and ctx stop the call through that cancel, and a call they stopped answers an error matching both cancelled
// and ctx's cause.
func tracked[A any](
	ctx context.Context,
	k *Kartta,
	method string,
	cancelled wire.Code,
	named func(id uint64) (any, wire.CancelParams),
) (*A, error) {
	if err := context.Cause(ctx); err != nil {
		if k.closed() {
			return nil, ErrClosed
		}
		return nil, err
	}
	id := k.ids.Add(1)
	params, cancel := named(id)
	ctx, stopCall := context.WithCancelCause(ctx)
	defer stopCall(nil)
	if !k.track(id, stopCall) {
		return nil, ErrClosed
	}
	defer k.untrack(id)

	k.mu.RLock()
	defer k.mu.RUnlock()
	if k.handle == nil {
		return nil, ErrClosed
	}
	handle := k.handle
	cancelSent := make(chan struct{})
	stop := context.AfterFunc(ctx, func() {
		defer close(cancelSent)
		_ = call(handle, "cancel", cancel, nil)
	})
	defer func() {
		if !stop() {
			<-cancelSent
		}
	}()
	var answer A
	if err := call(handle, method, params, &answer); err != nil {
		if cause := context.Cause(ctx); cause != nil && errors.Is(err, cancelled) {
			return nil, fmt.Errorf("%w: %w", err, cause)
		}
		return nil, err
	}
	return &answer, nil
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
	return ask[wire.Stats](k, "stats", nil)
}

// Version answers the library's ABI version, its crate version, the methods it answers and the languages it ships.
func (k *Kartta) Version() (*wire.Version, error) {
	return ask[wire.Version](k, "version", nil)
}

// Callers answers the callers of the symbol params name, transitively, in each graph that holds one.
func (k *Kartta) Callers(params wire.WalkParams) (*wire.Matches[wire.Walk], error) {
	return ask[wire.Matches[wire.Walk]](k, "callers", params)
}

// CallPath answers a chain of calls from one symbol to another, in each graph that holds both.
func (k *Kartta) CallPath(params wire.CallPathParams) (*wire.Matches[wire.CallPath], error) {
	return ask[wire.Matches[wire.CallPath]](k, "call_path", params)
}

// References answers the sites that call or use the symbol params name, in each graph that holds one.
func (k *Kartta) References(params wire.ReferencesParams) (*wire.Matches[wire.References], error) {
	return ask[wire.Matches[wire.References]](k, "references", params)
}

// Definitions answers the definitions the name params give selects, in each graph that holds one.
func (k *Kartta) Definitions(params wire.DefinitionsParams) (*wire.Matches[wire.Definitions], error) {
	return ask[wire.Matches[wire.Definitions]](k, "definitions", params)
}

// Sources answers the definitions the name params give selects, each with its text as its file holds it now.
func (k *Kartta) Sources(params wire.SourcesParams) (*wire.Matches[wire.Definitions], error) {
	return ask[wire.Matches[wire.Definitions]](k, "sources", params)
}

// Search answers the definitions that best match the words of a query, in each graph.
func (k *Kartta) Search(params wire.SearchParams) (*wire.Matches[wire.Search], error) {
	return ask[wire.Matches[wire.Search]](k, "search", params)
}

// Implementations answers the types that implement or extend the type params name, in each graph that holds one.
func (k *Kartta) Implementations(params wire.ImplementationsParams) (*wire.Matches[wire.Implementations], error) {
	return ask[wire.Matches[wire.Implementations]](k, "implementations", params)
}

// Outline answers the definitions of one file.
func (k *Kartta) Outline(params wire.OutlineParams) (*wire.Outline, error) {
	return ask[wire.Outline](k, "outline", params)
}

// Dependents answers the files that include a file, directly or through others.
func (k *Kartta) Dependents(params wire.DependentsParams) (*wire.Dependents, error) {
	return ask[wire.Dependents](k, "dependents", params)
}

// AnalyzeSegment answers what a change to a range of a file's lines reaches.
func (k *Kartta) AnalyzeSegment(params wire.SegmentParams) (*wire.Segment, error) {
	return ask[wire.Segment](k, "analyze_segment", params)
}

// AnalyzeChange answers what a change to regions across files reaches.
func (k *Kartta) AnalyzeChange(params wire.ChangeParams) (*wire.Change, error) {
	return ask[wire.Change](k, "analyze_change", params)
}

// List answers the entries under a directory of the index. A path the index does not hold answers wire.NotFound,
// and one that is not a directory wire.NotADirectory.
func (k *Kartta) List(params wire.ListParams) (*wire.Listing, error) {
	return ask[wire.Listing](k, "list", params)
}

// Glob answers the paths of the index the globs match. A glob that does not compile answers wire.InvalidGlob.
func (k *Kartta) Glob(params wire.GlobParams) (*wire.Glob, error) {
	return ask[wire.Glob](k, "glob", params)
}

// Fuzzy answers the paths of the index that best match a query.
func (k *Kartta) Fuzzy(params wire.FuzzyParams) (*wire.Fuzzy, error) {
	return ask[wire.Fuzzy](k, "fuzzy", params)
}

// Grep answers the lines of the indexed files the patterns match. A pattern that does not compile answers
// wire.InvalidPattern, and a glob wire.InvalidGlob.
func (k *Kartta) Grep(params wire.GrepParams) (*wire.Grep, error) {
	return ask[wire.Grep](k, "grep", params)
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

func ask[A any](k *Kartta, method string, params any) (*A, error) {
	var answer A
	if err := k.Call(method, params, &answer); err != nil {
		return nil, err
	}
	return &answer, nil
}

func bytesOf(b []byte) *C.uint8_t {
	return (*C.uint8_t)(unsafe.Pointer(unsafe.SliceData(b)))
}

func bytesAt(s C.KarttaString) []byte {
	return unsafe.Slice((*byte)(unsafe.Pointer(s.ptr)), int(s.len))
}
