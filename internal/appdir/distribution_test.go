package appdir

import (
	"path/filepath"
	"testing"
)

func TestDistributionKeepsItsOwnConfigAndCache(t *testing.T) {
	old := Name
	Name = "token-proxy-magpie"
	t.Cleanup(func() { Name = old })
	UseExecutable("")
	root := t.TempDir()
	t.Setenv("XDG_CONFIG_HOME", root)
	t.Setenv("XDG_CACHE_HOME", root)
	want := filepath.Join(root, Name)
	if Config() != want || Cache() != want || installed() != want {
		t.Fatalf("distribution directories must be isolated: %q, %q, %q", Config(), Cache(), installed())
	}
}
