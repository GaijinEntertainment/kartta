package wire

// Code is the kind of failure an Error reports, as the wire spells it. A Code is an error, and an Error unwraps to
// its Code, so errors.Is(err, wire.NotBuilt) matches every Error with that code. A library newer than this package may
// answer a code it does not name.
type Code string

// The codes the engine answers.
const (
	// InvalidRequest: a null pointer, bytes that are not UTF-8, or params that are not JSON.
	InvalidRequest Code = "invalid_request"
	// InvalidParams: params that are JSON but not the method's, such as an unknown field or language.
	InvalidParams Code = "invalid_params"
	// UnknownMethod: no method of that name; the message lists the methods.
	UnknownMethod Code = "unknown_method"
	// InvalidRoot: the root Open was given is not a directory that can be read.
	InvalidRoot Code = "invalid_root"
	// NotBuilt: the method needs a build and none has finished, or the last one failed or was cancelled.
	NotBuilt Code = "not_built"
	// Building: the method needs a build and one is running.
	Building Code = "building"
	// BuildFailed: the build stopped on a failure that leaves no index.
	BuildFailed Code = "build_failed"
	// BuildCancelled: a cancel stopped the build, which leaves no index.
	BuildCancelled Code = "build_cancelled"
	// Internal: the engine panicked; the handle stays usable.
	Internal Code = "internal"
)

func (c Code) Error() string {
	return string(c)
}

// Error is a failure the engine answered: a code to branch on, and a message for a reader.
type Error struct {
	Code    Code   `json:"code"`
	Message string `json:"message"`
}

func (e *Error) Error() string {
	return string(e.Code) + ": " + e.Message
}

// Unwrap returns the error's Code.
func (e *Error) Unwrap() error {
	return e.Code
}
