package gui

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/provider"
)

// #2008: the editor knows nothing of the time of day a provider's daily
// allowance comes back, set by `magpie provider quota-rest <id> until
// 08:00`, nor of its rest cap (`magpie provider rest-cap`), so its save
// keeps both.
func TestSaveKeepsQuotaResetAt(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "reset-test", Name: "Reset", Chat: "https://example.invalid/v1", Key: "k"}); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetQuotaResetAt("reset-test", "00:00+08:00"); err != nil {
		t.Fatal(err)
	}
	if err := provider.SetRestCap("reset-test", 240); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	providerRoutes(mux, nil)
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("POST", "/api/provider/save", strings.NewReader(`{"id":"reset-test","name":"Reset","chat":"https://example.invalid/v1"}`)))
	if w.Code != 200 {
		t.Fatalf("%d %s", w.Code, w.Body)
	}
	got, err := provider.Find("reset-test")
	if err != nil {
		t.Fatal(err)
	}
	if got.QuotaResetAt != "00:00+08:00" || got.RestCap != 240 || got.Name != "Reset" {
		t.Fatalf("the save dropped the reset time %q or the rest cap %d", got.QuotaResetAt, got.RestCap)
	}
}

// The editor's Failure rest row (restcap.js) sends restCap once it is
// changed: -1 for no rest, 0 for the gateway's own, minutes for a cap. A
// save without it keeps what was set, and one out of range is refused
// with nothing written.
func TestSaveSetsRestCap(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	t.Setenv("XDG_CACHE_HOME", t.TempDir())
	if err := provider.Save(provider.Provider{ID: "cap-test", Name: "Cap", Chat: "https://example.invalid/v1", Key: "k"}); err != nil {
		t.Fatal(err)
	}
	mux := http.NewServeMux()
	providerRoutes(mux, nil)
	save := func(extra string) int {
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest("POST", "/api/provider/save", strings.NewReader(`{"id":"cap-test","name":"Cap","chat":"https://example.invalid/v1"`+extra+`}`)))
		return w.Code
	}
	restCap := func() int {
		got, err := provider.Find("cap-test")
		if err != nil {
			t.Fatal(err)
		}
		return got.RestCap
	}
	for _, c := range []struct {
		extra string
		code  int
		want  int
	}{
		{`,"restCap":-1`, 200, provider.RestCapOff},
		{``, 200, provider.RestCapOff},
		{`,"restCap":15`, 200, 15},
		{`,"restCap":99999`, 500, 15},
		{`,"restCap":-2`, 500, 15},
		{``, 200, 15},
		{`,"restCap":0`, 200, 0},
	} {
		if code := save(c.extra); (code == 200) != (c.code == 200) {
			t.Fatalf("save %q answered %d", c.extra, code)
		}
		if got := restCap(); got != c.want {
			t.Fatalf("after save %q the rest cap is %d, want %d", c.extra, got, c.want)
		}
	}
}
