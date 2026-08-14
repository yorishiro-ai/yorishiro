import type { FormEvent } from "react";
import { useEffect, useState } from "react";
import { addMember, getTenantOverview } from "@/lib/api";
import type { MemberRecord, TenantOverview } from "@/types/api";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/Card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/Table";
import { Badge } from "@/components/ui/Badge";
import { Button } from "@/components/ui/Button";
import { Input } from "@/components/ui/Input";
import { PageSkeleton } from "@/components/ui/Skeleton";
import { cn } from "@/lib/cn";

type MemberRole = "viewer" | "member" | "admin" | "owner";

const ROLE_OPTIONS: MemberRole[] = ["viewer", "member", "admin", "owner"];

const ROLE_BADGE_VARIANT: Record<MemberRole, "default" | "secondary" | "outline" | "destructive"> =
  {
    owner: "destructive",
    admin: "default",
    member: "secondary",
    viewer: "outline",
  };

export function MembersPage() {
  const [overview, setOverview] = useState<TenantOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [newEmail, setNewEmail] = useState("");
  const [newRole, setNewRole] = useState<MemberRole>("member");
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [submitSuccess, setSubmitSuccess] = useState<string | null>(null);

  async function loadOverview() {
    setLoading(true);
    setError(null);
    try {
      const data = await getTenantOverview();
      setOverview(data);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load members");
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    loadOverview();
  }, []);

  async function handleAddMember(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (!newEmail.trim()) return;

    setSubmitting(true);
    setSubmitError(null);
    setSubmitSuccess(null);
    try {
      await addMember(newEmail.trim(), newRole);
      setSubmitSuccess(`${newEmail.trim()} was added as ${newRole}.`);
      setNewEmail("");
      setNewRole("member");
      await loadOverview();
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : "Failed to add member");
    } finally {
      setSubmitting(false);
    }
  }

  if (loading) {
    return <PageSkeleton />;
  }

  if (error) {
    return (
      <div className="p-6">
        <Card>
          <CardContent className="pt-6">
            <p className="text-sm text-destructive">{error}</p>
            <Button className="mt-4" size="sm" onClick={loadOverview}>
              Retry
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (!overview) {
    return null;
  }

  const members: MemberRecord[] = overview.members;

  return (
    <div className="space-y-6 p-6">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight text-foreground">Members</h1>
        <p className="text-sm text-muted-foreground">
          Manage your organization's members and roles.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>All members</CardTitle>
        </CardHeader>
        <CardContent>
          <div className="overflow-x-auto">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Email</TableHead>
                  <TableHead>Name</TableHead>
                  <TableHead>Role</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {members.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={3} className="text-center text-muted-foreground">
                      No members yet
                    </TableCell>
                  </TableRow>
                ) : (
                  members.map((member) => (
                    <TableRow key={member.user_id}>
                      <TableCell>{member.email}</TableCell>
                      <TableCell>{member.display_name ?? "—"}</TableCell>
                      <TableCell>
                        <Badge variant={ROLE_BADGE_VARIANT[member.role]}>{member.role}</Badge>
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Add member</CardTitle>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleAddMember} className="flex flex-col gap-4 sm:flex-row sm:items-end">
            <div className="flex-1">
              <Input
                label="Email"
                type="email"
                name="email"
                placeholder="member@example.com"
                value={newEmail}
                onChange={(e) => setNewEmail(e.target.value)}
                required
              />
            </div>
            <div className="w-full sm:w-48">
              <label htmlFor="role" className="mb-1 block text-sm font-medium text-foreground">
                Role
              </label>
              <select
                id="role"
                name="role"
                value={newRole}
                onChange={(e) => setNewRole(e.target.value as MemberRole)}
                className={cn(
                  "w-full rounded-md border border-input bg-card px-3 py-2 text-sm text-foreground shadow-sm",
                  "focus:outline-none focus:ring-2 focus:ring-ring",
                )}
              >
                {ROLE_OPTIONS.map((role) => (
                  <option key={role} value={role}>
                    {role}
                  </option>
                ))}
              </select>
            </div>
            <Button type="submit" disabled={submitting}>
              {submitting ? "Adding…" : "Add member"}
            </Button>
          </form>
          {submitSuccess && <p className="mt-3 text-sm text-link">{submitSuccess}</p>}
          {submitError && <p className="mt-3 text-sm text-destructive">{submitError}</p>}
        </CardContent>
      </Card>
    </div>
  );
}
