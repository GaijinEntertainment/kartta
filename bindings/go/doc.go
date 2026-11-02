// Package kartta calls the kartta code graph engine through its C ABI, linked statically through cgo.
//
// A Kartta is a handle on one tree. Build walks the tree once and builds the file index and the graphs of the
// languages its options name; Refresh brings them up to the tree in place, or a Build with Watch set keeps them there
// from a thread of the library's own until the next Build or Close; every other method asks what they hold, and
// answers wire types. A Kartta is safe for use by several goroutines at once: queries run concurrently, builds one at
// a time, a query waits for a refresh under way, and Close waits for the calls in flight.
//
// The module lives in bindings/go of the kartta repository, so its version vX.Y.Z is the tag bindings/go/vX.Y.Z, on the
// commit of the release vX.Y.Z. The package compiles against the kartta.h the module holds, and links libkartta_ffi.a
// of the same version: the GitHub release vX.Y.Z carries one archive per target, and in a checkout cargo rustc --locked
// --profile dist -p kartta-ffi --crate-type staticlib builds the same archive into target/dist. Name the archive's
// directory in CGO_LDFLAGS:
//
//	go get dev.gaijin.team/go/kartta@vX.Y.Z
//	CGO_LDFLAGS=-L<directory of libkartta_ffi.a> go build
//
// On Windows cgo needs MinGW-w64's gcc, whose linker refuses the x86_64-pc-windows-msvc archive, so the module links
// libkartta_ffi.a from the release's kartta-ffi-X.Y.Z-x86_64-pc-windows-gnu.tar.gz. In a checkout, cargo build -p
// kartta-ffi --target x86_64-pc-windows-gnu builds it into target/x86_64-pc-windows-gnu/debug.
package kartta
