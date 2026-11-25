package provider

import (
	"context"
	"encoding/json"
	"errors"
	"github.com/yetone/magpie/internal/catalog"
)

// A migrated account belongs to this gateway, even when Codex itself uses
// an API key. Never write it into the agent's own auth.json to make it visible.
func codexSavedFirst() (savedLogin, bool) {
	for _, l := range sharedLogins() {
		if !l.TauriImported || l.Agent != "codex" || !l.On || l.Hidden != "" {
			continue
		}
		var a codexAuth
		if json.Unmarshal(l.Auth, &a) == nil && a.Tokens.AccessToken != "" {
			return l, true
		}
	}
	return savedLogin{}, false
}

func codexSavedStandIn() (Provider, bool) {
	l, ok := codexSavedFirst()
	if !ok {
		return Provider{}, false
	}
	user := l.User
	a := &Account{Agent: "codex", User: user, Plan: l.Plan, Stream: true, standIn: true}
	a.token = func(ctx context.Context) (string, error) {
		if l.Lapsed != "" {
			return "", errors.New(l.Lapsed)
		}
		tok, _, err := savedLoginToken(ctx, "codex", user)
		return tok, err
	}
	a.sign = codexSign(func(ctx context.Context) (string, string, error) {
		if l.Lapsed != "" {
			return "", "", errors.New(l.Lapsed)
		}
		return savedLoginToken(ctx, "codex", user)
	})
	a.renew = func(ctx context.Context, stale string) (string, string, error) {
		if l.Lapsed != "" {
			return "", "", errors.New(l.Lapsed)
		}
		return renewSavedLoginFrom(ctx, "codex", user, stale)
	}
	a.body = codexBody
	a.models = catalog.Codex
	a.listedOnly = true
	a.fetch = func(ctx context.Context) ([]catalog.Model, error) {
		ms, ver, err := codexModelsAsked(ctx, a.sign)
		if err != nil {
			return nil, err
		}
		if err := catalog.SaveLiveAsked(accountModels("codex", user), CodexBase, ver, ms); err != nil {
			return nil, err
		}
		codexFetchSaved(ctx)
		ms = codexPoolModels(ms)
		return ms, catalog.SaveLive("codex", CodexBase, ms)
	}
	return Provider{ID: "codex", Name: "Codex", Icon: "codex-color", Responses: CodexBase, Website: "https://chatgpt.com/codex", Account: a}, true
}
