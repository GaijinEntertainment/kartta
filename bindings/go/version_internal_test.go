package kartta

import (
	"errors"
	"strings"
	"testing"
)

func Test_matchVersion(t *testing.T) {
	tests := []struct {
		name        string
		giveLibrary string
		giveBinding string
		wantRefused bool
	}{
		{name: "the same version opens", giveLibrary: "0.1.0", giveBinding: "0.1.0"},
		{name: "another patch version opens", giveLibrary: "0.1.3", giveBinding: "0.1.0"},
		{name: "another minor version is refused", giveLibrary: "0.2.0", giveBinding: "0.1.0", wantRefused: true},
		{name: "another major version is refused", giveLibrary: "1.1.0", giveBinding: "0.1.0", wantRefused: true},
		{name: "a version with no patch field is refused", giveLibrary: "0.1", giveBinding: "0.1", wantRefused: true},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			err := matchVersion(tt.giveLibrary, tt.giveBinding)
			if !tt.wantRefused {
				if err != nil {
					t.Fatalf("matchVersion(%q, %q) = %v, want nil", tt.giveLibrary, tt.giveBinding, err)
				}
				return
			}
			if !errors.Is(err, ErrABI) {
				t.Fatalf("matchVersion(%q, %q) = %v, want ErrABI", tt.giveLibrary, tt.giveBinding, err)
			}
			for _, named := range []string{tt.giveLibrary, tt.giveBinding} {
				if !strings.Contains(err.Error(), "version "+named) && !strings.Contains(err.Error(), "package "+named) {
					t.Errorf("the error %q does not name %q", err, named)
				}
			}
		})
	}
}

func Test_openVersion(t *testing.T) {
	root := t.TempDir()
	t.Run("a binding of the library's minor version opens", func(t *testing.T) {
		k, err := open(root, version)
		if err != nil {
			t.Fatal(err)
		}
		_ = k.Close()
	})
	t.Run("a binding of another minor version is refused", func(t *testing.T) {
		if _, err := open(root, "99.0.0"); !errors.Is(err, ErrABI) {
			t.Errorf("open = %v, want ErrABI", err)
		}
	})
}
