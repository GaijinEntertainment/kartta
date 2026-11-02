package wire_test

import (
	"encoding/json"
	"testing"

	"dev.gaijin.team/go/kartta/wire"
)

// No golden holds stopped_build: a request on the fixture cannot stop a build partway, so each answer that can carry it
// is decoded here.
func TestAnswersReadFromAStoppedBuildKeepItsAccount(t *testing.T) {
	t.Parallel()

	const answer = `{"stopped_build":{"entries":3,"unvisited":2}}`
	read := map[string]func([]byte) (*wire.StoppedBuild, error){
		"Listing": decoded(func(v *wire.Listing) *wire.StoppedBuild { return v.StoppedBuild }),
		"Glob":    decoded(func(v *wire.Glob) *wire.StoppedBuild { return v.StoppedBuild }),
		"Fuzzy":   decoded(func(v *wire.Fuzzy) *wire.StoppedBuild { return v.StoppedBuild }),
		"Grep":    decoded(func(v *wire.Grep) *wire.StoppedBuild { return v.StoppedBuild }),
		"Stats":   decoded(func(v *wire.Stats) *wire.StoppedBuild { return v.StoppedBuild }),
	}

	for name, read := range read {
		got, err := read([]byte(answer))
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if got == nil || *got != (wire.StoppedBuild{Entries: 3, Unvisited: 2}) {
			t.Errorf("%s: StoppedBuild = %+v", name, got)
		}
	}
}

func decoded[T any](field func(*T) *wire.StoppedBuild) func([]byte) (*wire.StoppedBuild, error) {
	return func(answer []byte) (*wire.StoppedBuild, error) {
		var value T
		if err := json.Unmarshal(answer, &value); err != nil {
			return nil, err
		}
		return field(&value), nil
	}
}
