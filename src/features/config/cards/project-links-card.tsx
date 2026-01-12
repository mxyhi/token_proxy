import { openUrl } from "@tauri-apps/plugin-opener";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { TOKEN_PROXY_GITHUB_URL } from "@/lib/links";
import { m } from "@/paraglide/messages.js";

export function ProjectLinksCard() {
  return (
    <Card data-slot="project-links-card">
      <CardHeader>
        <CardTitle>{m.project_links_title()}</CardTitle>
        <CardDescription>{m.project_links_desc()}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 text-sm">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs uppercase tracking-[0.2em] text-muted-foreground">
              {m.project_links_github_label()}
            </p>
            <p className="mt-1 truncate font-mono text-xs text-foreground/80">
              {TOKEN_PROXY_GITHUB_URL}
            </p>
          </div>
          <Button
            type="button"
            variant="outline"
            onClick={() => {
              void openUrl(TOKEN_PROXY_GITHUB_URL);
            }}
          >
            {m.project_links_open()}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
