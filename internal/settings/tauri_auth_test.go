package settings

import "testing"

func TestLocalKeyRequirementStaysOnThisMachine(t *testing.T) {
	incoming := Settings{}
	incoming.KeepOwn(Settings{RequireLocalKey: true})
	if !incoming.RequireLocalKey {
		t.Fatal("sync removed local authentication")
	}
	incoming.KeepOwn(Settings{})
	if incoming.RequireLocalKey {
		t.Fatal("sync imported another machine's authentication mode")
	}
}
