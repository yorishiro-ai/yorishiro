import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { getSetupStatus, setup } from "@/lib/api";
import { useAuth } from "@/hooks/useAuth";
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from "@/components/ui/Card";
import { Input } from "@/components/ui/Input";
import { Button } from "@/components/ui/Button";

/**
 * First-run setup for a self-hosted deployment: creates the first tenant, its workspace and the
 * owner account in one step.
 *
 * A hosted deployment never shows this. The server gates `POST /setup` on the tenant cap being
 * finite, and a hosted process sets that cap to unlimited, so the wizard is off there and its own
 * checkout or invite flow onboards a tenant instead.
 */
export function SetupPage() {
  const navigate = useNavigate();
  const { loginSession } = useAuth();

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [checking, setChecking] = useState(true);

  // A deployment that is already set up, or one where the wizard is disabled, must not sit on a
  // form that can only fail. Both cases send the visitor to the login page instead.
  useEffect(() => {
    let cancelled = false;
    getSetupStatus()
      .then((status) => {
        if (cancelled) return;
        if (!status.setup_required) navigate("/login", { replace: true });
        else setChecking(false);
      })
      .catch(() => {
        if (!cancelled) setChecking(false);
      });
    return () => {
      cancelled = true;
    };
  }, [navigate]);

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      const result = await setup(email, password, displayName || undefined);
      loginSession(result.api_key, result.email, result.workspace_id);
      navigate("/dashboard", { replace: true });
    } catch (err) {
      const status = (err as { status?: number }).status;
      // 409 and 404 both mean the form can never succeed: someone else finished setup first, or
      // the wizard is disabled on this deployment. Say which, and point at the login page.
      if (status === 409) {
        setError("This deployment has already been set up. Sign in instead.");
      } else if (status === 404) {
        setError("The setup wizard is not enabled on this deployment.");
      } else {
        setError(err instanceof Error ? err.message : "Setup failed");
      }
    } finally {
      setSubmitting(false);
    }
  }

  if (checking) return null;

  return (
    <div className="flex min-h-screen items-center justify-center bg-background px-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle>Set up Yorishiro</CardTitle>
          <CardDescription>
            Create the owner account for this deployment. This is asked once.
          </CardDescription>
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
              autoComplete="new-password"
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            <Input
              label="Display name (optional)"
              type="text"
              name="displayName"
              autoComplete="name"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
            />
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
            <Button type="submit" className="w-full" disabled={submitting}>
              {submitting ? "Setting up..." : "Create owner account"}
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
