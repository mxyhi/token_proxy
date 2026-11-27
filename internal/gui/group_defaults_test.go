package gui

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
)

// The Routing groups page's Settings card sets what every group follows
// when it sets nothing of its own (jiezhengj on Discord); the page reads
// it back in /api/groups, a bad value is refused, and the groups and the
// rest of the settings are left as they were.
func TestGroupDefaultsSaved(t *testing.T) {
	t.Setenv("HOME", t.TempDir())
	t.Setenv("USERPROFILE", os.Getenv("HOME"))
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	s := settings.Load()
	s.ClassifyTimeout = 9
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if err := provider.Save(provider.Provider{ID: "a", Name: "a", Key: "ka", Chat: "http://127.0.0.1:1/v1", Models: []string{"m1"}}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SaveGroup(provider.Group{ID: "own", Name: "Own", Members: []string{"a/m1"}, FirstToken: provider.FirstTokenWait}); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	groupRoutes(mux)
	post := func(body string) (int, groupsJSON) {
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest("POST", "/api/groups/defaults", strings.NewReader(body)))
		var out groupsJSON
		if w.Code == 200 {
			if err := json.Unmarshal(w.Body.Bytes(), &out); err != nil {
				t.Fatal(err)
			}
		}
		return w.Code, out
	}
	code, got := post(`{"sink":true,"firstToken":60,"patience":-1}`)
	if code != 200 || got.Defaults != (groupDefaultsJSON{Sink: true, FirstToken: 60, Patience: -1}) {
		t.Fatalf("%d %+v", code, got.Defaults)
	}
	s = settings.Load()
	if !s.GroupSink || s.GroupFirstToken != 60 || s.GroupPatience != -1 || s.ClassifyTimeout != 9 {
		t.Fatalf("settings: %v %d %d %d", s.GroupSink, s.GroupFirstToken, s.GroupPatience, s.ClassifyTimeout)
	}
	if g, _, _ := provider.FindGroup("group/own"); g.FirstToken != provider.FirstTokenWait || g.FirstTokenOr() > 0 || !g.Sinks() || g.Waits() != 0 {
		t.Fatalf("group: %+v", g)
	}
	if st := groupsState(); st.Defaults != (groupDefaultsJSON{Sink: true, FirstToken: 60, Patience: -1}) {
		t.Fatalf("state: %+v", st.Defaults)
	}
	if code, _ := post(`{"patience":999}`); code == 200 {
		t.Fatal("patience 999 saved")
	}
	if s := settings.Load(); s.GroupPatience != -1 {
		t.Fatalf("after a refusal: %d", s.GroupPatience)
	}
	// how the groups magpie finds route and stay; a group the user made
	// keeps its own, and a routing a found group can't follow is refused
	code, got = post(`{"routing":"order","affinity":"off"}`)
	if code != 200 || got.Defaults != (groupDefaultsJSON{Routing: "order", Affinity: "off"}) {
		t.Fatalf("found: %d %+v", code, got.Defaults)
	}
	if s := settings.Load(); s.FoundRouting != "order" || s.FoundAffinity != "off" || s.FoundRoutedAt == 0 || s.ClassifyTimeout != 9 {
		t.Fatalf("found settings: %q %q %d %d", s.FoundRouting, s.FoundAffinity, s.FoundRoutedAt, s.ClassifyTimeout)
	}
	if g, _, _ := provider.FindGroup("group/own"); g.Routing != "" || g.Affinity != "" {
		t.Fatalf("the user's group: %q %q", g.Routing, g.Affinity)
	}
	for _, bad := range []string{`{"routing":"weight"}`, `{"routing":"manual"}`, `{"affinity":"nope"}`} {
		if code, _ := post(bad); code == 200 {
			t.Fatalf("%s saved", bad)
		}
	}
	if s := settings.Load(); s.FoundRouting != "order" || s.FoundAffinity != "off" {
		t.Fatalf("after a refusal: %q %q", s.FoundRouting, s.FoundAffinity)
	}
	// The new built-in strategy offered by the picker saves as a default too.
	code, got = post(`{"routing":"pace-drain","affinity":"off"}`)
	if code != 200 || got.Defaults != (groupDefaultsJSON{Routing: "pace-drain", Affinity: "off"}) {
		t.Fatalf("pace-drain: %d %+v", code, got.Defaults)
	}
	// back to magpie's own
	if code, got := post(`{}`); code != 200 || got.Defaults != (groupDefaultsJSON{}) {
		t.Fatalf("cleared: %d %+v", code, got.Defaults)
	}
}
