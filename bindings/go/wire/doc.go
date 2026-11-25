// Package wire holds the JSON the kartta engine's C ABI reads and writes: each method's params, each answer, and the
// error codes, as Go types. It uses no cgo, so code that only handles answers, and its tests, build without the
// engine's library; the kartta package next to it makes the calls.
//
// Field names follow the wire. An optional params field left at its zero value is left out, and a bound left out cuts
// nothing. A field whose 0 asks for something else than leaving it out is a pointer, as Limit, so nil leaves it out and
// a pointer to 0 sends 0; each field's doc says what its zero value asks for. A bound the engine takes as an argument
// is required, and leaving it out answers InvalidParams naming it.
//
// An answer's count the wire leaves out at 0 decodes as 0. A count that is a pointer, as Matched, is counted only where
// its doc says, and nil there means not counted.
package wire
