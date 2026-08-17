import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { login, getOAuthStatus, getSetupStatus, ApiError } from "@/lib/api";
import type { ValidationDetail } from "@/lib/api";
import { useAuth } from "@/hooks/useAuth";
import { setSessionWorkspaceId, whoami } from "@/lib/api";
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
  CardFooter,
} from "@/components/ui/Card";
import { Input } from "@/components/ui/Input";
import { Button } from "@/components/ui/Button";

export function LoginPage() {
  const navigate = useNavigate();
  const { loginSession } = useAuth();

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [workspaceId, setWorkspaceId] = useState("");
  const [needsWorkspace, setNeedsWorkspace] = useState(false);
  const [workspaceChoices, setWorkspaceChoices] = useState<ValidationDetail[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [ssoEnabled, setSsoEnabled] = useState(false);

  useEffect(() => {
    getOAuthStatus()
      .then((status) => setSsoEnabled(status.enabled))
      .catch(() => setSsoEnabled(false));
  }, []);

  // A deployment with no tenant yet has no account to sign in with, so the first visitor is sent
  // to the wizard rather than left at a form that cannot succeed. Hosted deployments report
  // false here (their tenant cap is unlimited) and never redirect.
  useEffect(() => {
    let cancelled = false;
    getSetupStatus()
      .then((status) => {
        if (!cancelled && status.setup_required) navigate("/setup", { replace: true });
      })
      .catch(() => {
        // An unreachable status endpoint should not block a login that might still work.
      });
    return () => {
      cancelled = true;
    };
  }, [navigate]);

  useEffect(() => {
    const hash = window.location.hash;
    if (!hash) return;

    if (hash.includes("api_key=")) {
      const params = new URLSearchParams(hash.replace(/^#/, ""));
      const apiKey = params.get("api_key");
      const oauthEmail = params.get("email") ?? "";
      if (apiKey) {
        loginSession(apiKey, oauthEmail);
        window.history.replaceState(null, "", window.location.pathname);
        // The callback returns only the key, so the key's own workspace is asked for
        // separately -- `request` needs it to know when the workspace header is required.
        whoami()
          .then((who) => setSessionWorkspaceId(who.workspace_id))
          .catch(() => {
            // Leaving it unset only means the header is sent when it need not be, which a
            // workspace-scoped key refuses -- visible, not silent.
          });
        navigate("/dashboard", { replace: true });
      }
    } else if (hash.includes("error=oauth_failed") || hash.includes("error=access_denied")) {
      setError("SSO login failed. Please try again or use email/password.");
      window.history.replaceState(null, "", window.location.pathname);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      const result = await login(email, password, needsWorkspace ? workspaceId : undefined);
      loginSession(result.api_key, email, result.workspace_id);
      navigate("/dashboard");
    } catch (err) {
      if (err instanceof ApiError && err.status === 422) {
        setNeedsWorkspace(true);
        // `details` is only present against a server new enough to send workspace candidates;
        // an older one (or a malformed body) leaves it null, and the text input below is the
        // fallback for that case.
        if (err.details && err.details.length > 0) {
          setWorkspaceChoices(err.details);
          setWorkspaceId(err.details[0].field);
          setError("Multiple workspaces found. Please choose one.");
        } else {
          setWorkspaceChoices(null);
          setError("Multiple workspaces found. Please specify a workspace ID.");
        }
      } else {
        const message = err instanceof Error ? err.message : "Login failed";
        setError(message);
      }
    } finally {
      setSubmitting(false);
    }
  }

  function handleSsoClick() {
    window.location.href = "/auth/oauth/authorize";
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-background px-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle>Sign in</CardTitle>
          <CardDescription>Sign in to your Yorishiro workspace.</CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="space-y-4">
            <Input
              label="Email"
              type="email"
              name="email"
              autoComplete="email"
              required
              value={email}
              onChange={(e) => setEmail(e.target.value)}
            />
            <Input
              label="Password"
              type="password"
              name="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            {needsWorkspace && workspaceChoices && (
              <div className="w-full">
                <label htmlFor="workspaceId" className="mb-1 block text-sm font-medium">
                  Workspace
                </label>
                <select
                  id="workspaceId"
                  name="workspaceId"
                  required
                  value={workspaceId}
                  onChange={(e) => setWorkspaceId(e.target.value)}
                  className="w-full rounded-md border border-input bg-card px-3 py-2 text-sm shadow-sm focus:outline-none focus:ring-2 focus:ring-ring"
                >
                  {workspaceChoices.map((choice) => (
                    <option key={choice.field} value={choice.field}>
                      {choice.problem}
                    </option>
                  ))}
                </select>
              </div>
            )}
            {needsWorkspace && !workspaceChoices && (
              <Input
                label="Workspace ID"
                type="text"
                name="workspaceId"
                required
                value={workspaceId}
                onChange={(e) => setWorkspaceId(e.target.value)}
              />
            )}
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
            <Button type="submit" className="w-full" disabled={submitting}>
              {submitting ? "Signing in..." : "Sign in"}
            </Button>
          </form>

          {ssoEnabled && (
            <>
              <div className="my-4 flex items-center gap-3">
                <div className="h-px flex-1 bg-border" />
                <span className="text-xs text-muted-foreground">OR</span>
                <div className="h-px flex-1 bg-border" />
              </div>
              <Button type="button" variant="secondary" className="w-full" onClick={handleSsoClick}>
                Sign in with SSO
              </Button>
            </>
          )}
        </CardContent>
        <CardFooter className="justify-center">
          <p className="text-sm text-muted-foreground">
            Don&apos;t have an account?{" "}
            <Link to="/signup" className="font-medium text-link hover:underline">
              Sign up
            </Link>
          </p>
        </CardFooter>
      </Card>
    </div>
  );
}
