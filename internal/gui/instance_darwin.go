package gui

import (
	"log"
	"os"
	"time"

	"github.com/yetone/magpie/internal/proc"
	"github.com/yetone/magpie/internal/settings"
	"github.com/yetone/magpie/internal/update"
)

// runningAlready is whether another magpie, this same one on this config,
// is running; then this launch hands over to it and ends. Wails has no
// single instance on the Mac, and magpie can be started twice there: by
// its own open at login (a LaunchAgent that runs the binary) and by a
// Login Item the user added, or by the Mac reopening it after a shutdown
// it wasn't quit for (Crispin on Discord: two magpies after each boot).
// A start at login just ends; one for the window has the running magpie
// show its window (the Dock's reopen), and a magpie:// link is opened,
// which the Mac hands to the running one.
func runningAlready(showMain bool, link string) bool {
	exe, err := os.Executable()
	if err != nil {
		return false
	}
	lock := instanceLock(settings.Dir(), exe)
	held, err := holdInstance(lock)
	if err != nil {
		log.Println("one magpie:", err)
		return false
	}
	if held {
		return false
	}
	log.Println("magpie is running already; handing over to it")
	switch {
	case link != "":
		proc.Command("open", link).Run()
	case showMain:
		// once in a while at most, so an open that started another of
		// these (were the running one not known to the Mac) can't go on
		stamp := lock + ".shown"
		if fi, err := os.Stat(stamp); err == nil && time.Since(fi.ModTime()) < 10*time.Second {
			break
		}
		os.WriteFile(stamp, nil, 0o600)
		os.Chtimes(stamp, time.Now(), time.Now())
		// by its bundle id: the magpie running may be another copy of the
		// app, which opening this one's path wouldn't reach
		if update.Bundle() != "" {
			proc.Command("open", "-b", "com.mxyhi.token-proxy.magpie").Run()
		}
	}
	return true
}
