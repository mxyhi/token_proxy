// Package tpmigrate converts a Tauri configuration offline. It never edits the
// source, connects an agent, refreshes credentials or merges into a live store.
package tpmigrate

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/tidwall/jsonc"
	"github.com/yetone/magpie/internal/access"
	"github.com/yetone/magpie/internal/provider"
	"github.com/yetone/magpie/internal/settings"
	_ "github.com/yetone/magpie/internal/sqlitedb"
)

type config struct {
	Host     string `json:"host"`
	Port     int    `json:"port"`
	Proxy    string `json:"app_proxy_url"`
	Strategy struct {
		Order    string `json:"order"`
		Dispatch struct {
			Type string `json:"type"`
		} `json:"dispatch"`
	} `json:"upstream_strategy"`
	Upstreams []upstream `json:"upstreams"`
	Keys      []localKey `json:"local_api_keys"`
}
type upstream struct {
	ID         string            `json:"id"`
	Providers  []string          `json:"providers"`
	Base       string            `json:"base_url"`
	Proxy      string            `json:"proxy_url"`
	Priority   int               `json:"priority"`
	Enabled    bool              `json:"enabled"`
	Models     []string          `json:"available_models"`
	Mappings   map[string]string `json:"model_mappings"`
	Credential struct {
		Type      string   `json:"type"`
		Keys      []string `json:"api_keys"`
		Provider  string   `json:"provider"`
		AccountID string   `json:"account_id"`
	} `json:"credential"`
}
type localKey struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Key     string `json:"key"`
	Enabled bool   `json:"enabled"`
	Scope   struct {
		Type string   `json:"type"`
		IDs  []string `json:"upstream_ids"`
	} `json:"scope"`
}

// Tauri's serde default is enabled; omission must not silently disable one.
func (u *upstream) UnmarshalJSON(b []byte) error {
	type plain upstream
	v := plain{Enabled: true}
	if err := json.Unmarshal(b, &v); err != nil {
		return err
	}
	*u = upstream(v)
	return nil
}
func (k *localKey) UnmarshalJSON(b []byte) error {
	type plain localKey
	v := plain{Enabled: true}
	if err := json.Unmarshal(b, &v); err != nil {
		return err
	}
	*k = localKey(v)
	return nil
}

type providerFile struct {
	Providers    []provider.Provider `json:"providers"`
	Groups       []provider.Group    `json:"groups"`
	NoAutoGroups bool                `json:"noAutoGroups"`
	Order        []string            `json:"order"`
}
type login struct {
	ID         string          `json:"id"`
	Agent      string          `json:"agent"`
	User       string          `json:"user"`
	On         bool            `json:"on"`
	Seen       time.Time       `json:"seen"`
	Auth       json.RawMessage `json:"auth"`
	AccessOnly bool            `json:"accessOnly,omitempty"`
}
type report struct {
	Upstreams     int      `json:"upstreams"`
	Enabled       int      `json:"enabled"`
	Keys          int      `json:"keys"`
	CodexAccounts int      `json:"codexAccounts"`
	Groups        int      `json:"groups"`
	Port          int      `json:"port"`
	Warnings      []string `json:"warnings"`
}
type plan struct {
	providers providerFile
	settings  settings.Settings
	keys      []access.Key
	logins    []login
	report    report
	original  []byte
}

// Run defaults to inspection. --apply writes a new directory only, after every
// input is validated; credentials are never written to stdout or an error.
func Run(args []string, out io.Writer) error {
	f := flag.NewFlagSet("migrate-tauri", flag.ContinueOnError)
	f.SetOutput(out)
	source := f.String("source", "", "旧版目录，含 config.jsonc 和 data.db")
	dest := f.String("output", "", "新配置目录，必须不存在")
	apply := f.Bool("apply", false, "写入新目录；默认仅检查")
	if err := f.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return nil
		}
		return err
	}
	if *source == "" || f.NArg() != 0 {
		return errors.New("用法: token-proxy migrate-tauri --source <旧目录> [--output <新目录> --apply]")
	}
	p, err := prepare(*source)
	if err != nil {
		return err
	}
	if *apply {
		if *dest == "" {
			return errors.New("--apply 必须指定 --output")
		}
		if err = p.write(*dest); err != nil {
			return err
		}
	}
	return json.NewEncoder(out).Encode(p.report)
}

func prepare(source string) (*plan, error) {
	b, err := os.ReadFile(filepath.Join(source, "config.jsonc"))
	if err != nil {
		return nil, errors.New("无法读取旧版 config.jsonc")
	}
	var c config
	if json.Unmarshal(jsonc.ToJSON(b), &c) != nil {
		return nil, errors.New("旧版配置不是有效 JSONC 或字段类型不受支持")
	}
	p, err := convert(c)
	if err != nil {
		return nil, err
	}
	p.original = b
	for _, u := range c.Upstreams {
		if u.Credential.Type != "account" {
			continue
		}
		if len(p.logins) > 0 {
			return nil, errors.New("当前迁移仅支持一个 Codex 账户上游；拒绝合并账户路由")
		}
		l, err := readCodex(source, u.Credential.AccountID, u.Enabled)
		if err != nil {
			return nil, err
		}
		p.logins = append(p.logins, l)
	}
	p.report.CodexAccounts = len(p.logins)
	return p, nil
}

func convert(c config) (*plan, error) {
	if c.Port == 0 {
		c.Port = 9208
	}
	if err := settings.CheckPort(c.Port); err != nil {
		return nil, err
	}
	if c.Host != "0.0.0.0" && c.Host != "127.0.0.1" && c.Host != "localhost" {
		return nil, errors.New("迁移仅支持 0.0.0.0 或 loopback 监听地址")
	}
	if c.Strategy.Order != "fill_first" || c.Strategy.Dispatch.Type != "serial" {
		return nil, errors.New("迁移仅支持 fill_first + serial；其他调度策略需要先明确映射")
	}
	if err := settings.CheckProxy(c.Proxy); err != nil {
		return nil, errors.New("旧版全局代理无效")
	}
	p := &plan{providers: providerFile{NoAutoGroups: true}, settings: settings.Settings{Port: c.Port, LAN: c.Host == "0.0.0.0", Proxy: c.Proxy, RequireLocalKey: len(c.Keys) > 0, ModelWires: map[string]string{}}}
	p.report = report{Upstreams: len(c.Upstreams), Keys: len(c.Keys), Port: c.Port, Warnings: []string{
		"原始 config.jsonc 保留为 tauri-config.original.jsonc；历史用量数据库另行备份，不转换为新版统计。",
		"协议转换使用新基座规则；旧 convert_from_map 白名单、超时、请求体上限、CORS、热模型展示与会话冷却设置保留在原始配置，未等价移植。",
		"裸模型路由按迁移时的模型列表生成，按旧优先级降序串行回退；后续增加模型或修改上游需同步编辑路由组和密钥别名。",
	}}
	us := slices.Clone(c.Upstreams)
	slices.SortStableFunc(us, func(a, b upstream) int {
		if a.Priority > b.Priority {
			return -1
		}
		if a.Priority < b.Priority {
			return 1
		}
		return 0
	})
	ids := map[string]string{}
	for _, u := range us {
		if u.ID == "" || strings.ContainsAny(u.ID, "/\\\r\n") || ids[u.ID] != "" {
			return nil, errors.New("上游 ID 为空、重复或含路径分隔符")
		}
		id := u.ID
		if u.Credential.Type == "account" {
			if u.Credential.Provider != "codex" || !slices.Equal(u.Providers, []string{"codex"}) {
				return nil, errors.New("当前仅支持 Codex OAuth 账户迁移")
			}
			id = "codex"
		}
		if slices.Contains(p.providers.Order, id) {
			return nil, errors.New("迁移后的上游 ID 冲突")
		}
		ids[u.ID] = id
		if err := settings.CheckProxy(u.Proxy); err != nil {
			return nil, errors.New("上游代理无效")
		}
		v := provider.Provider{ID: id, Name: u.ID, Proxy: u.Proxy, Off: !u.Enabled, Models: slices.Clone(u.Models), Routing: provider.Ordered, Affinity: provider.AffinityOff, NoBalanceProbe: true}
		switch u.Credential.Type {
		case "api_keys":
			if len(u.Credential.Keys) == 0 {
				return nil, errors.New("API key 上游缺少密钥")
			}
			v.Key = u.Credential.Keys[0]
			for _, k := range u.Credential.Keys[1:] {
				v.Keys = append(v.Keys, provider.KeyAccount{Key: k})
			}
		case "passthrough":
			if u.Enabled {
				return nil, errors.New("启用中的 passthrough 暂不支持迁移")
			}
			p.report.Warnings = append(p.report.Warnings, "禁用的 passthrough 已保留禁用占位；重新启用前须配置凭据。")
		case "account":
		default:
			return nil, errors.New("不支持的上游凭据类型")
		}
		if u.Credential.Type != "account" {
			base, err := baseURL(u.Base)
			if err != nil {
				return nil, err
			}
			for _, proto := range u.Providers {
				switch proto {
				case "openai":
					v.Chat = versionBase(base, "v1")
				case "openai-response":
					v.Responses = versionBase(base, "v1")
				case "anthropic":
					v.Anthropic = strings.TrimSuffix(base, "/v1")
				case "gemini":
					v.Gemini = versionBase(base, "v1beta")
				default:
					return nil, errors.New("不支持的上游协议")
				}
			}
		}
		if u.Enabled {
			p.report.Enabled++
			if len(u.Models) == 0 {
				return nil, errors.New("启用的上游缺少明确模型列表，无法保证裸模型路由")
			}
		}
		for from, to := range u.Mappings {
			p.settings.ModelWires[id+"/"+from] = to
		}
		p.providers.Providers = append(p.providers.Providers, v)
		p.providers.Order = append(p.providers.Order, id)
	}
	localAuto := false
	secrets := map[string]bool{}
	keyIDs := map[string]bool{}
	for _, k := range c.Keys {
		if k.Key == "" || secrets[k.Key] || k.ID == "" || keyIDs[k.ID] {
			return nil, errors.New("本地密钥为空或重复")
		}
		secrets[k.Key] = true
		keyIDs[k.ID] = true
		if k.Scope.Type != "auto" && k.Scope.Type != "selected" {
			return nil, errors.New("不支持的本地密钥作用域")
		}
		for _, id := range k.Scope.IDs {
			if ids[id] == "" {
				return nil, errors.New("密钥作用域引用了不存在的上游")
			}
		}
		key := access.Key{ID: k.ID, Name: k.Name, Secret: k.Key, Off: !k.Enabled, Aliases: map[string]string{}}
		members := map[string][]string{}
		for _, u := range us {
			if k.Scope.Type == "selected" && !slices.Contains(k.Scope.IDs, u.ID) {
				continue
			}
			if !u.Enabled {
				continue
			}
			id := ids[u.ID]
			key.Models = append(key.Models, id+"/*")
			for _, m := range u.Models {
				members[m] = append(members[m], id+"/"+m)
				if id != u.ID {
					key.Aliases[u.ID+"/"+m] = id + "/" + m
				}
			}
		}
		names := make([]string, 0, len(members))
		for m := range members {
			names = append(names, m)
		}
		slices.Sort(names)
		for _, m := range names {
			gid := shortID(k.ID + "\x00" + m)
			p.providers.Groups = append(p.providers.Groups, provider.Group{ID: gid, Name: m, Members: members[m], Routing: provider.Ordered, Affinity: provider.AffinityOff, NoSink: true, Patience: provider.PatienceOff})
			ref := "group/" + gid
			key.Aliases[m] = ref
			key.Models = append(key.Models, ref)
		}
		// An empty allow-list means unrestricted in the new store. Keep an empty
		// selected scope denied, including when all its upstreams are disabled.
		if len(key.Models) == 0 {
			key.Models = []string{"group/tp-no-permitted-models"}
		}
		if _, err := access.CleanAliases(key.Aliases); err != nil {
			return nil, errors.New("模型别名数量或格式超出新基座限制；拒绝截断")
		}
		if k.Enabled && (p.settings.LocalKeyID == "" || k.Scope.Type == "auto" && !localAuto) {
			p.settings.LocalKeyID = k.ID
			localAuto = k.Scope.Type == "auto"
		}
		p.keys = append(p.keys, key)
	}
	if len(c.Keys) == 0 {
		return nil, errors.New("无本地密钥的旧配置需先创建密钥，才能保留裸模型路由")
	}
	p.report.Groups = len(p.providers.Groups)
	return p, nil
}
func shortID(s string) string {
	h := sha256.Sum256([]byte(s))
	return "tp-" + hex.EncodeToString(h[:12])
}
func baseURL(s string) (string, error) {
	u, err := url.Parse(s)
	if err != nil || u.Host == "" || (u.Scheme != "https" && u.Scheme != "http") || u.RawQuery != "" || u.Fragment != "" {
		return "", errors.New("上游 URL 必须为 HTTP(S) base URL，不得包含查询或 fragment")
	}
	return strings.TrimRight(s, "/"), nil
}
func versionBase(s, v string) string {
	if strings.HasSuffix(s, "/"+v) {
		return s
	}
	return s + "/" + v
}

func (p *plan) write(dest string) error {
	dest, err := filepath.Abs(dest)
	if err != nil {
		return err
	}
	if _, err := os.Lstat(dest); !errors.Is(err, os.ErrNotExist) {
		return errors.New("目标目录已存在或无法检查；拒绝覆盖")
	}
	if err := os.MkdirAll(filepath.Dir(dest), 0700); err != nil {
		return err
	}
	stage, err := os.MkdirTemp(filepath.Dir(dest), ".tp-migration-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(stage)
	files := map[string]any{"providers.json": p.providers, "settings.json": p.settings, "caller-keys.json": p.keys, "logins.json": p.logins, "tauri-migration-report.json": p.report}
	for name, value := range files {
		b, err := json.MarshalIndent(value, "", "  ")
		if err != nil {
			return err
		}
		if err = os.WriteFile(filepath.Join(stage, name), b, 0600); err != nil {
			return err
		}
	}
	if err = os.WriteFile(filepath.Join(stage, "tauri-config.original.jsonc"), p.original, 0600); err != nil {
		return err
	}
	return os.Rename(stage, dest)
}
