import { useEffect, useState } from "react";
import { Terminal, ExternalLink, KeyRound } from "lucide-react";
import { whoami } from "@/lib/api";
import type { WhoAmIResponse } from "@/types/api";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/Card";
import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { PageSkeleton } from "@/components/ui/Skeleton";

const SCOPE_BADGE_VARIANT: Record<string, "default" | "secondary" | "outline" | "destructive"> = {
  admin: "destructive",
  write: "default",
  read: "secondary",
};

const CLI_COMMANDS = [
  {
    command: "admin create-api-key <workspace-id> <scope>",
    description: "Create a new API key for a workspace with the given scope (read, write, admin).",
  },
  {
    command: "admin list-api-keys <workspace-id>",
    description: "List all API keys issued for a workspace.",
  },
  {
    command: "admin revoke-api-key <key-id>",
    description: "Revoke an existing API key by its ID.",
  },
];

export function ApiKeysPage() {
  const [session, setSession] = useState<WhoAmIResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  async function loadSession() {
    setLoading(true);
    setError(null);
    try {
      const data = await whoami();
      setSession(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load session information");
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    loadSession();
  }, []);

  if (loading) {
    return <PageSkeleton />;
  }

  return (
    <div className="space-y-6 p-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">API Keys</h1>
        <p className="text-sm text-muted-foreground">
          API key management and current session information.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Managed via Admin CLI</CardTitle>
          <CardDescription>
            API keys are created, listed, and revoked using the admin CLI, not this web console.
            There is no REST endpoint for managing keys: only the current session's identity can be
            inspected here.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <a
            href="/docs"
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-1 text-sm text-link hover:underline"
          >
            View API documentation
            <ExternalLink className="h-3.5 w-3.5" />
          </a>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <KeyRound className="h-4 w-4 text-muted-foreground" />
            <CardTitle className="text-lg">Current Session</CardTitle>
          </div>
          <CardDescription>
            The workspace, tenant, and scope associated with the API key used for this session.
          </CardDescription>
        </CardHeader>
        <CardContent>
          {error && (
            <div className="mb-4 rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              {error}
              <Button className="ml-3" size="sm" variant="secondary" onClick={loadSession}>
                Retry
              </Button>
            </div>
          )}

          {session && (
            <dl className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
              <div>
                <dt className="text-xs font-medium text-muted-foreground">Workspace ID</dt>
                <dd className="mt-1 break-all font-mono text-sm">{session.workspace_id}</dd>
              </div>
              <div>
                <dt className="text-xs font-medium text-muted-foreground">Tenant ID</dt>
                <dd className="mt-1 break-all font-mono text-sm">{session.tenant_id}</dd>
              </div>
              <div>
                <dt className="text-xs font-medium text-muted-foreground">Scope</dt>
                <dd className="mt-1">
                  <Badge variant={SCOPE_BADGE_VARIANT[session.scope] ?? "outline"}>
                    {session.scope}
                  </Badge>
                </dd>
              </div>
              <div>
                <dt className="text-xs font-medium text-muted-foreground">User ID</dt>
                <dd className="mt-1 break-all font-mono text-sm">
                  {session.user_id ?? "— (key not bound to a user)"}
                </dd>
              </div>
            </dl>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <div className="flex items-center gap-2">
            <Terminal className="h-4 w-4 text-muted-foreground" />
            <CardTitle className="text-lg">CLI Reference</CardTitle>
          </div>
          <CardDescription>
            Run these commands from a machine with access to the admin CLI and database.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <ul className="space-y-3">
            {CLI_COMMANDS.map((entry) => (
              <li key={entry.command} className="rounded-md border bg-muted/50 p-3">
                <code className="block break-all font-mono text-sm text-foreground">
                  {entry.command}
                </code>
                <p className="mt-1 text-sm text-muted-foreground">{entry.description}</p>
              </li>
            ))}
          </ul>
        </CardContent>
      </Card>
    </div>
  );
}
