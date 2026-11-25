package gateway

import (
	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/settings"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestTauriLocalAuthentication(t *testing.T) {
	fresh(t)
	s := settings.Load()
	s.RequireLocalKey = true
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	key, err := access.Update("add-key", access.Change{Name: "Migrated"})
	if err != nil {
		t.Fatal(err)
	}
	h := lanGuard(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
	call := func(path, token string) int {
		r := httptest.NewRequest("GET", "http://127.0.0.1"+path, nil)
		r.RemoteAddr = "127.0.0.1:12345"
		if token != "" {
			r.Header.Set("Authorization", "Bearer "+token)
		}
		w := httptest.NewRecorder()
		h.ServeHTTP(w, r)
		return w.Code
	}
	for _, path := range []string{"/v1/models", "/v1/chat/completions", "/v1/responses", "/v1/messages", "/v1beta/models", "/models", "/chat/completions", "/responses", "/messages"} {
		for _, bad := range []string{"", "wrong", "magpie"} {
			if got := call(path, bad); got != 401 {
				t.Fatalf("%s accepted missing/invalid key: %d", path, got)
			}
		}
		if got := call(path, key); got != 204 {
			t.Fatalf("%s refused enabled key: %d", path, got)
		}
	}
	keys, _ := access.List()
	if _, err = access.Update("off-key", access.Change{Key: keys[0].ID}); err != nil {
		t.Fatal(err)
	}
	if got := call("/v1/models", key); got != 401 {
		t.Fatal("disabled key accepted", got)
	}
	if got := call("/", ""); got != 204 {
		t.Fatal("health probe requires key", got)
	}
}
