package main

import (
	"github.com/yetone/magpie/internal/appdir"
	"github.com/yetone/magpie/internal/gateway"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/testenv"
	"github.com/yetone/magpie/internal/update"
	"testing"
)

func TestDistributionGatewayPort(t *testing.T) {
	testenv.SetHome(t, t.TempDir())
	appdir.UseExecutable("")
	oldName, oldPort, oldManual := appdir.Name, settings.DefaultPort, update.ManualOnly
	t.Cleanup(func() { appdir.Name = oldName; settings.DefaultPort = oldPort; update.ManualOnly = oldManual })
	t.Setenv("MAGPIE_ADDR", "")
	configureDistribution()
	if got := gateway.Addr(); got != "127.0.0.1:9208" {
		t.Fatalf("default gateway=%s; want TP port 9208", got)
	}
	s := settings.Load()
	s.Port = 19208
	if err := settings.Save(s); err != nil {
		t.Fatal(err)
	}
	if got := gateway.Addr(); got != "127.0.0.1:19208" {
		t.Fatal("saved port lost", got)
	}
	t.Setenv("MAGPIE_ADDR", "127.0.0.1:19209")
	if got := gateway.Addr(); got != "127.0.0.1:19209" {
		t.Fatal("explicit address lost", got)
	}
}
