// Package wire holds the JSON the kartta engine's C ABI reads and writes: each method's params, each answer, and the
// error codes, as Go types. It uses no cgo, so code that only handles answers, and its tests, build without the
// engine's library; the kartta package next to it makes the calls.
//
// Field names follow the wire. An optional params field left at its zero value is left out, and the engine takes its
// default; each field's doc says what its zero value asks for.
package wire
