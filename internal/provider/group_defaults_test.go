package provider

import (
	"testing"
	"time"

	"github.com/yetone/magpie/internal/settings"
)

// What a group sets wins; what it leaves unset follows the default for
// every group in settings (jiezhengj on Discord), and with that unset too,
// magpie's own: wait for a first token, keep one's place, a minute's
// patience.
func TestGroupDefaultsUnlessItsOwn(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	none := Group{}
	if none.Waits() != PatienceDefault*time.Second || none.FirstTokenOr() != 0 || none.Sinks() {
		t.Fatalf("magpie's own: %s %d %v", none.Waits(), none.FirstTokenOr(), none.Sinks())
	}
	if err := settings.SetGroupDefaults(settings.GroupDefaults{Sink: true, FirstToken: 45, Patience: 200}); err != nil {
		t.Fatal(err)
	}
	if none.Waits() != 200*time.Second || none.FirstTokenOr() != 45 || !none.Sinks() {
		t.Fatalf("default: %s %d %v", none.Waits(), none.FirstTokenOr(), none.Sinks())
	}
	own := Group{Patience: PatienceDefault, FirstToken: FirstTokenWait, NoSink: true}
	if own.Waits() != PatienceDefault*time.Second || own.FirstTokenOr() > 0 || own.Sinks() {
		t.Fatalf("own: %s %d %v", own.Waits(), own.FirstTokenOr(), own.Sinks())
	}
	if err := settings.SetGroupDefaults(settings.GroupDefaults{Patience: PatienceOff}); err != nil {
		t.Fatal(err)
	}
	if none.Waits() != 0 {
		t.Fatalf("default off: %s", none.Waits())
	}
	if g := (Group{Patience: 120, Sink: true, FirstToken: 30}); g.Waits() != 120*time.Second || !g.Sinks() || g.FirstTokenOr() != 30 {
		t.Fatalf("own over off: %s %v %d", g.Waits(), g.Sinks(), g.FirstTokenOr())
	}
	// a value no setting can hold is refused, and the file kept
	for _, bad := range [][2]int{{-2, 0}, {0, -2}, {0, PatienceLongest + 1}} {
		if err := settings.SetGroupDefaults(settings.GroupDefaults{FirstToken: bad[0], Patience: bad[1]}); err == nil {
			t.Fatalf("%v saved", bad)
		}
	}
	if none.Waits() != 0 {
		t.Fatalf("after refusals: %s", none.Waits())
	}

	// Wait, a minute and Keeps its place are kept as the group's own
	if err := Save(Provider{ID: "a", Name: "a", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"x"}}); err != nil {
		t.Fatal(err)
	}
	if err := SaveGroup(Group{Name: "Own", Members: []string{"a/x"}, Routing: Ordered, FirstToken: FirstTokenWait, Patience: PatienceDefault, NoSink: true}); err != nil {
		t.Fatal(err)
	}
	g, _, _ := FindGroup("group/own")
	if g.FirstToken != FirstTokenWait || g.Patience != PatienceDefault || !g.NoSink {
		t.Fatalf("saved: %+v", g)
	}
	// Sink and NoSink both: Sink
	if err := SaveGroup(Group{ID: "own", Name: "Own", Members: []string{"a/x"}, Routing: Ordered, Sink: true, NoSink: true}); err != nil {
		t.Fatal(err)
	}
	if g, _, _ := FindGroup("group/own"); !g.Sink || g.NoSink {
		t.Fatalf("both: %+v", g)
	}
}

// The groups magpie finds route and keep a conversation as settings say
// (token_proxy: 自动组按顺序、关闭会话保持); a group the user made keeps its
// own, a routing change stamps when, so conversations move at their next
// turn, and a routing a found group can't follow is refused.
func TestFoundGroupsFollowTheirDefaults(t *testing.T) {
	autoRefHome(t)
	if err := SaveGroup(Group{Name: "Mine", Members: []string{"a/x", "b/x"}}); err != nil {
		t.Fatal(err)
	}
	found := func() (Group, Group) {
		t.Helper()
		var auto, mine Group
		for _, g := range Groups() {
			switch g.ID {
			case "auto-claude-opus-5-5":
				auto = g
			case "mine":
				mine = g
			}
		}
		if auto.ID == "" || mine.ID == "" {
			t.Fatalf("groups: %q %q", auto.ID, mine.ID)
		}
		return auto, mine
	}
	if auto, _ := found(); auto.Routing != "" || auto.Affinity != "" || !auto.RoutedAt.IsZero() {
		t.Fatalf("magpie's own: %q %q %s", auto.Routing, auto.Affinity, auto.RoutedAt)
	}
	if err := settings.SetGroupDefaults(settings.GroupDefaults{FoundRouting: Ordered, FoundAffinity: AffinityOff}); err != nil {
		t.Fatal(err)
	}
	at := time.UnixMilli(settings.Load().FoundRoutedAt)
	if settings.Load().FoundRoutedAt == 0 {
		t.Fatal("a routing change kept no time")
	}
	auto, mine := found()
	if auto.Routing != Ordered || auto.Affinity != AffinityOff || !auto.RoutedAt.Equal(at) {
		t.Fatalf("found: %q %q %s, want %s", auto.Routing, auto.Affinity, auto.RoutedAt, at)
	}
	if !GroupRoutedAt("auto-claude-opus-5-5").Equal(at) {
		t.Fatalf("GroupRoutedAt: %s", GroupRoutedAt("auto-claude-opus-5-5"))
	}
	if g, _, ok := FindGroup("auto/claude-opus-5.5"); !ok || g.Routing != Ordered || g.Affinity != AffinityOff {
		t.Fatalf("FindGroup: %v %q %q", ok, g.Routing, g.Affinity)
	}
	if mine.Routing != "" || mine.Affinity != "" || !mine.RoutedAt.IsZero() {
		t.Fatalf("the user's group: %q %q %s", mine.Routing, mine.Affinity, mine.RoutedAt)
	}
	// stays alone changed: the routing's time is kept
	if err := settings.SetGroupDefaults(settings.GroupDefaults{FoundRouting: Ordered, FoundAffinity: AffinityTurn}); err != nil {
		t.Fatal(err)
	}
	if s := settings.Load(); s.FoundRoutedAt != at.UnixMilli() || s.FoundAffinity != AffinityTurn {
		t.Fatalf("stays changed: %d %q", s.FoundRoutedAt, s.FoundAffinity)
	}
	for _, bad := range []settings.GroupDefaults{{FoundRouting: Weighted}, {FoundRouting: Manual}, {FoundRouting: "smart"}, {FoundRouting: "nope"}, {FoundAffinity: "nope"}} {
		if err := settings.SetGroupDefaults(bad); err == nil {
			t.Fatalf("%+v saved", bad)
		}
	}
	if auto, _ := found(); auto.Routing != Ordered || auto.Affinity != AffinityTurn {
		t.Fatalf("after refusals: %q %q", auto.Routing, auto.Affinity)
	}
	// Groups of one follow the same defaults when that option is on.
	if err := Save(Provider{ID: "solo", Name: "Solo", Key: "k", Chat: "http://127.0.0.1:1/v1", Models: []string{"single-model"}}); err != nil {
		t.Fatal(err)
	}
	if err := SetAutoGroupsOfOne(true); err != nil {
		t.Fatal(err)
	}
	if g, ms, ok := FindGroup("auto/single-model"); !ok || len(ms) != 1 || g.Routing != Ordered || g.Affinity != AffinityTurn || !g.RoutedAt.Equal(at) {
		t.Fatalf("found group of one: %v %d %q %q %s", ok, len(ms), g.Routing, g.Affinity, g.RoutedAt)
	}
	if err := SetAutoGroupsOfOne(false); err != nil {
		t.Fatal(err)
	}
	if auto, _ := found(); auto.Routing != Ordered || auto.Affinity != AffinityTurn || !auto.RoutedAt.Equal(at) {
		t.Fatalf("groups of one off changed defaults: %q %q %s", auto.Routing, auto.Affinity, auto.RoutedAt)
	}
	// back to Smart and Auto
	if err := settings.SetGroupDefaults(settings.GroupDefaults{}); err != nil {
		t.Fatal(err)
	}
	if auto, _ := found(); auto.Routing != "" || auto.Affinity != "" || auto.RoutedAt.Before(at) {
		t.Fatalf("cleared: %q %q %s", auto.Routing, auto.Affinity, auto.RoutedAt)
	}
}
