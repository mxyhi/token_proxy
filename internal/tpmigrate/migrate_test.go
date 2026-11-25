package tpmigrate

import (
	"bytes"
	"database/sql"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
)

func TestMain(m *testing.M) { testenv.Main(m) }

const fixture = `{
 // comments and trailing commas are intentional
 "host":"0.0.0.0", "port":9208,
 "upstream_strategy":{"order":"fill_first","dispatch":{"type":"serial"}},
 "local_api_keys":[
  {"id":"main","name":"main","key":"caller-secret","enabled":true,"scope":{"type":"auto"}},
  {"id":"limited","key":"disabled-secret","enabled":false,"scope":{"type":"selected","upstream_ids":["low"]}}
 ],
 "upstreams":[
  {"id":"low","providers":["openai","openai-response"],"base_url":"https://low.invalid","credential":{"type":"api_keys","api_keys":["low-secret","second-secret"]},"priority":1,"enabled":true,"available_models":["deepseek-flash","vendor/model"]},
  {"id":"high","providers":["anthropic","gemini"],"base_url":"https://high.invalid","credential":{"type":"api_keys","api_keys":["high-secret"]},"priority":99,"enabled":true,"available_models":["deepseek-flash"],"model_mappings":{"deepseek-flash":"wire-model"}},
  {"id":"off","providers":["openai"],"base_url":"https://off.invalid","credential":{"type":"api_keys","api_keys":["off-secret"]},"priority":999,"enabled":false,"available_models":["deepseek-flash"]},
 ]
}`

func sourceFixture(t *testing.T) string {
	t.Helper()
	d := t.TempDir()
	if err := os.WriteFile(filepath.Join(d, "config.jsonc"), []byte(fixture), 0600); err != nil {
		t.Fatal(err)
	}
	return d
}
func TestMigrationRoutingAndAuthentication(t *testing.T) {
	source := sourceFixture(t)
	p, err := prepare(source)
	if err != nil {
		t.Fatal(err)
	}
	if p.report.Enabled != 2 || !p.settings.RequireLocalKey || p.settings.Port != 9208 || !p.settings.LAN {
		t.Fatal("listener and auth changed")
	}
	if p.settings.ModelWires["high/deepseek-flash"] != "wire-model" {
		t.Fatal("lost upstream model mapping")
	}
	for _, g := range p.providers.Groups {
		if g.Name == "deepseek-flash" && g.ID == strings.TrimPrefix(p.keys[0].Aliases[g.Name], "group/") {
			if !slices.Equal(g.Members, []string{"high/deepseek-flash", "low/deepseek-flash"}) || g.Routing != provider.Ordered {
				t.Fatal("priority or disabled state changed")
			}
		}
	}
	for _, g := range p.providers.Groups {
		if g.ID == strings.TrimPrefix(p.keys[1].Aliases[g.Name], "group/") && !slices.Equal(g.Members, []string{"low/" + g.Name}) {
			t.Fatal("selected scope widened")
		}
	}
	if !p.keys[1].Off || p.keys[0].Secret != "caller-secret" {
		t.Fatal("key state changed")
	}
	// Read the generated files through the application's real readers.
	dest := filepath.Join(t.TempDir(), "data")
	if err := p.write(dest); err != nil {
		t.Fatal(err)
	}
	testenv.SetHome(t, t.TempDir())
	appdir.UseExecutable("")
	t.Setenv("XDG_CONFIG_HOME", filepath.Dir(dest))
	old := appdir.Name
	appdir.Name = "data"
	t.Cleanup(func() { appdir.Name = old })
	if access.LocalSecret() != "caller-secret" {
		t.Fatal("local integrations cannot authenticate")
	}
	if settings.Load().Port != 9208 {
		t.Fatal("settings not readable")
	}
	if _, err := access.Check("caller-secret"); err != nil {
		t.Fatal("imported key not accepted")
	}
	if _, err := access.Check("disabled-secret"); err == nil {
		t.Fatal("disabled key accepted")
	}
	got, model, ok := provider.Resolve("high/deepseek-flash")
	if !ok || model != "deepseek-flash" || provider.UpstreamName(got, model) != "wire-model" {
		t.Fatal("model routing cannot read generated provider")
	}
	for _, key := range p.keys {
		for _, ref := range key.Aliases {
			if _, _, ok := provider.Resolve(ref); !ok {
				t.Fatal("unresolvable imported alias", ref)
			}
		}
	}
}
func TestMigrationIsOfflineAndDoesNotOverwrite(t *testing.T) {
	source := sourceFixture(t)
	dest := filepath.Join(t.TempDir(), "target")
	var out bytes.Buffer
	if err := Run([]string{"--source", source, "--output", dest}, &out); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(dest); !os.IsNotExist(err) {
		t.Fatal("dry run wrote files")
	}
	if strings.Contains(out.String(), "secret") {
		t.Fatal("credential leaked")
	}
	if err := Run([]string{"--source", source, "--output", dest, "--apply"}, &out); err != nil {
		t.Fatal(err)
	}
	original, _ := os.ReadFile(filepath.Join(source, "config.jsonc"))
	if string(original) != fixture {
		t.Fatal("source changed")
	}
	before, _ := os.ReadFile(filepath.Join(dest, "providers.json"))
	if err := Run([]string{"--source", source, "--output", dest, "--apply"}, &out); err == nil {
		t.Fatal("overwrote target")
	}
	after, _ := os.ReadFile(filepath.Join(dest, "providers.json"))
	if !bytes.Equal(before, after) {
		t.Fatal("target changed on failure")
	}
	info, _ := os.Stat(filepath.Join(dest, "caller-keys.json"))
	if info.Mode().Perm() != 0600 {
		t.Fatal("unsafe credential permissions")
	}
}
func TestReadCodexUsesCurrentDatabase(t *testing.T) {
	dir := t.TempDir()
	db, err := sql.Open("sqlite", filepath.Join(dir, "data.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	if _, err = db.Exec("CREATE TABLE provider_accounts(provider_kind TEXT,account_id TEXT,record_json TEXT)"); err != nil {
		t.Fatal(err)
	}
	record := `{"account_id":"chatgpt-account","email":"test@example.invalid","credential":{"type":"oauth","access_token":"new-access","refresh_token":"new-refresh","id_token":"id-token","client_id":"app_EMoamEEZ73f0CkXaXp7hrann","auto_refresh_enabled":true,"last_refresh":"2026-11-25T00:00:00Z"}}`
	if _, err = db.Exec("INSERT INTO provider_accounts VALUES ('codex','bound-account',?)", record); err != nil {
		t.Fatal(err)
	}
	l, err := readCodex(dir, "bound-account", true)
	if err != nil {
		t.Fatal(err)
	}
	var auth struct {
		Tokens map[string]string `json:"tokens"`
	}
	if json.Unmarshal(l.Auth, &auth) != nil || auth.Tokens["access_token"] != "new-access" || !l.On {
		t.Fatal("lost current OAuth credentials")
	}
	if _, err = readCodex(dir, "missing-account", true); err == nil {
		t.Fatal("missing account guessed")
	}
}

func TestUnsupportedMigrationLeavesTargetAbsent(t *testing.T) {
	cases := map[string][2]string{
		"parallel":      {"serial", "parallel"},
		"passthrough":   {`"type":"api_keys","api_keys":["low-secret","second-secret"]`, `"type":"passthrough"`},
		"scope":         {`"type":"auto"`, `"type":"unknown"`},
		"duplicate key": {"disabled-secret", "caller-secret"},
	}
	for name, change := range cases {
		t.Run(name, func(t *testing.T) {
			source := sourceFixture(t)
			b := strings.ReplaceAll(fixture, change[0], change[1])
			if err := os.WriteFile(filepath.Join(source, "config.jsonc"), []byte(b), 0600); err != nil {
				t.Fatal(err)
			}
			dest := filepath.Join(t.TempDir(), "out")
			var out bytes.Buffer
			if err := Run([]string{"--source", source, "--output", dest, "--apply"}, &out); err == nil {
				t.Fatal("accepted unsupported configuration")
			}
			if _, err := os.Stat(dest); !os.IsNotExist(err) {
				t.Fatal("failed conversion wrote target")
			}
		})
	}
}

func TestTauriDefaultsRemainEnabled(t *testing.T) {
	var u upstream
	var k localKey
	if json.Unmarshal([]byte(`{"id":"u"}`), &u) != nil || !u.Enabled {
		t.Fatal("omitted upstream enabled became false")
	}
	if json.Unmarshal([]byte(`{"id":"k"}`), &k) != nil || !k.Enabled {
		t.Fatal("omitted key enabled became false")
	}
}
