// Package kartta calls the kartta code graph engine through its C ABI, linked statically through cgo.
//
// A Kartta is a handle on one tree. Build walks the tree once and builds the file index and the graphs of the
// languages its options name; Refresh brings them up to the tree in place; every other method asks what they hold,
// and answers wire types. A Kartta is safe for use by several goroutines at once: queries run concurrently, builds one
// at a time, a query waits for a refresh under way, and Close waits for the calls in flight.
//
// The package links libkartta_ffi.a, built from the same kartta commit by cargo build -p kartta-ffi, and compiles
// against crates/kartta-ffi/include/kartta.h of that checkout. Name the archive's directory in CGO_LDFLAGS:
//
//	CGO_LDFLAGS=-L<checkout>/target/release go build
package kartta
