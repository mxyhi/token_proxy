package tpmigrate

import (
	"database/sql"
	"encoding/json"
	"errors"
	"net/url"
	"path/filepath"
	"time"
)

type codexRecord struct {
	Status     string `json:"status"`
	AccountID  string `json:"account_id"`
	Email      string `json:"email"`
	Credential struct {
		Type        string `json:"type"`
		Access      string `json:"access_token"`
		Refresh     string `json:"refresh_token"`
		ID          string `json:"id_token"`
		Client      string `json:"client_id"`
		AutoRefresh *bool  `json:"auto_refresh_enabled"`
		LastRefresh string `json:"last_refresh"`
	} `json:"credential"`
}

func readCodex(source, id string, on bool) (login, error) {
	path, err := filepath.Abs(filepath.Join(source, "data.db"))
	if err != nil {
		return login{}, err
	}
	uri := url.URL{Scheme: "file", Path: filepath.ToSlash(path), RawQuery: "mode=ro"}
	db, err := sql.Open("sqlite", uri.String())
	if err != nil {
		return login{}, errors.New("无法打开旧版账户数据库")
	}
	defer db.Close()
	var raw string
	if db.QueryRow("SELECT record_json FROM provider_accounts WHERE provider_kind = 'codex' AND account_id = ?", id).Scan(&raw) != nil {
		return login{}, errors.New("无法读取绑定的 Codex 账户；未使用历史文件替代")
	}
	var a codexRecord
	if json.Unmarshal([]byte(raw), &a) != nil {
		return login{}, errors.New("Codex 账户记录无效")
	}
	c := a.Credential
	if c.Type != "oauth" || c.Access == "" || c.ID == "" || a.Email == "" || a.AccountID == "" {
		return login{}, errors.New("当前只支持完整的 Codex OAuth 记录")
	}
	if c.Client != "" && c.Client != "app_EMoamEEZ73f0CkXaXp7hrann" {
		return login{}, errors.New("Codex OAuth client 不兼容，拒绝使用错误 client 续期")
	}
	auth, err := json.Marshal(map[string]any{"tokens": map[string]string{"access_token": c.Access, "refresh_token": c.Refresh, "id_token": c.ID, "account_id": a.AccountID}, "last_refresh": c.LastRefresh})
	if err != nil {
		return login{}, err
	}
	lapsed := ""
	if a.Status == "invalid" {
		lapsed = "Imported Tauri Codex account is invalid; sign in again"
	}
	return login{Lapsed: lapsed, TauriImported: true, ID: id, Agent: "codex", User: a.Email, On: on, Seen: time.Now().UTC(), Auth: auth, AccessOnly: c.AutoRefresh != nil && !*c.AutoRefresh}, nil
}
