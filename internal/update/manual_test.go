package update

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
)

func TestManualDistributionNeverRequestsAnUpdate(t *testing.T) {
	old := ManualOnly
	ManualOnly = true
	t.Cleanup(func() { ManualOnly = old })
	var requests atomic.Int64
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests.Add(1)
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer srv.Close()
	t.Setenv("MAGPIE_UPDATE_FEED", srv.URL)
	for _, latest := range []func(context.Context) (*Release, error){
		Latest, LatestStable,
		func(ctx context.Context) (*Release, error) { return LatestEvery(ctx, "zh") },
	} {
		if rel, err := latest(context.Background()); rel != nil || err == nil || !strings.Contains(err.Error(), ManualReleases) {
			t.Fatalf("manual distribution should point to its own downloads: %v, %v", rel, err)
		}
	}
	if Released("0.2.0-prelease.1") || requests.Load() != 0 {
		t.Fatal("manual distribution must not schedule or fetch upstream updates")
	}
}
