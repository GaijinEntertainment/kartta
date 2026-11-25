package kartta

import (
	"fmt"
	"strings"
)

const version = "0.0.0"

// matchVersion answers nil when the library's version and the binding's share their major and minor versions, and
// ErrABI naming both otherwise, a version with no patch field included.
func matchVersion(library, binding string) error {
	if line := releaseLine(library); line == "" || line != releaseLine(binding) {
		return fmt.Errorf("%w: the library is version %s, the package %s", ErrABI, library, binding)
	}
	return nil
}

// releaseLine answers the major and minor fields of a version, or "" when it has no patch field.
func releaseLine(v string) string {
	major, rest, ok := strings.Cut(v, ".")
	if !ok {
		return ""
	}
	minor, _, ok := strings.Cut(rest, ".")
	if !ok {
		return ""
	}
	return major + "." + minor
}
