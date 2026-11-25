// magpie — one place to pick every agent's model. The commands live in
// internal/cli; this is only where the binary starts.
package main

import (
	_ "embed"

	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/cli"
	"github.com/yetone/magpie/internal/notices"
	"github.com/yetone/magpie/internal/update"
	// the built-in plugins, registered with the runtime (internal/pluginrt)
	_ "github.com/yetone/magpie/plugins/all"
)

// version is set at build time: -ldflags "-X main.version=…" (Makefile,
// Dockerfile, magpie-releases), so it stays in package main.
var version = "dev"

// the notices of what is compiled into every build (moejs's Intl data and
// code), carried by the binary itself, since most release assets are a
// bare binary
//
//go:embed THIRD_PARTY_NOTICES
var thirdParty string

func main() {
	// 新基座与官方 Magpie、旧版 Tauri 的配置及更新通道隔离。
	appdir.Name = "token-proxy-magpie"
	update.ManualOnly = true
	notices.Text = thirdParty
	cli.Main(version)
}
