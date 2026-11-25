package provider

import (
	"context"
	"encoding/json"
	"github.com/yetone/magpie/internal/testenv"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestTauriCodexWithoutClientLogin(t *testing.T) {
	testenv.SetHome(t, t.TempDir())
	auth := map[string]any{"tokens": map[string]string{"access_token": "saved-access", "refresh_token": "saved-refresh", "id_token": "saved-id", "account_id": "saved-account"}, "last_refresh": time.Now().UTC().Format(time.RFC3339)}
	raw, _ := json.Marshal(auth)
	if err := writeLogins([]savedLogin{{ID: "bound", Agent: "codex", User: "imported@example.invalid", On: true, Auth: raw, TauriImported: true, AccessOnly: true}}); err != nil {
		t.Fatal(err)
	}
	home, _ := os.UserHomeDir()
	p, ok := codexAccount(home)
	if !ok || p.Account == nil || p.Account.User != "imported@example.invalid" {
		t.Fatal("imported account requires a client login")
	}
	tok, used, err := p.Account.Token(context.Background())
	if err != nil || !used || tok != "saved-access" {
		t.Fatal("saved token not served")
	}
	if got := p.AlsoOn(); len(got) != 0 {
		t.Fatal("stand-in account also listed as its own fallback")
	}
	for _, e := range savedButSignedOut() {
		if e.Agent == "codex" {
			t.Fatal("usable import reported signed out")
		}
	}
	if _, err := os.Stat(filepath.Join(home, ".codex", "auth.json")); !os.IsNotExist(err) {
		t.Fatal("migration wrote the client's auth")
	}
	ls := readLogins()
	ls[0].Lapsed = "Imported account invalid"
	if err := writeLogins(ls); err != nil {
		t.Fatal(err)
	}
	p, ok = codexAccount(home)
	if !ok {
		t.Fatal("invalid account disappeared instead of reporting its state")
	}
	if _, _, err := p.Account.Token(context.Background()); err == nil {
		t.Fatal("invalid imported account served a token")
	}
	ls[0].On = false
	if err := writeLogins(ls); err != nil {
		t.Fatal(err)
	}
	if _, ok := codexAccount(home); ok {
		t.Fatal("disabled saved account served")
	}
}
