package settings

import "testing"

func TestLocalKeyRequirementStaysOnThisMachine(t *testing.T) {
	incoming := Settings{}
	incoming.KeepOwn(Settings{RequireLocalKey: true, LocalKeyID: "imported"})
	if !incoming.RequireLocalKey || incoming.LocalKeyID != "imported" {
		t.Fatal("sync removed local authentication")
	}
	incoming.KeepOwn(Settings{})
	if incoming.RequireLocalKey || incoming.LocalKeyID != "" {
		t.Fatal("sync imported another machine's authentication mode")
	}
}
