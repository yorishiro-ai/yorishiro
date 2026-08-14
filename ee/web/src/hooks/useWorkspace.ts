import { useCallback, useSyncExternalStore } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { createExternalStore } from "@/lib/externalStore";

const NAME_STORAGE_KEY = "yorishiro_workspace_name";

const { subscribe, notify } = createExternalStore();

function getNameSnapshot(): string | null {
  return sessionStorage.getItem(NAME_STORAGE_KEY);
}

export function useWorkspace() {
  const { wsId } = useParams<{ wsId: string }>();
  const navigate = useNavigate();
  const workspaceName = useSyncExternalStore(subscribe, getNameSnapshot);

  const workspaceId = wsId ?? null;
  const shortId = wsId ? wsId.split("-")[0] : null;

  const selectWorkspace = useCallback(
    (id: string, name: string) => {
      sessionStorage.setItem(NAME_STORAGE_KEY, name);
      notify();
      navigate(`/ws/${id}/dashboard`);
    },
    [navigate],
  );

  /// Forgets the remembered workspace *without* navigating.
  ///
  /// `TenantScope` calls this in an effect on every tenant-level route, so navigating from here
  /// sent all of them to the dashboard on a full page load — `/marketplace`, `/schemas/:id` and
  /// `/schemas/templates/:id` were unreachable by URL. In-app navigation hid it, because the
  /// effect does not re-run when only the child route changes.
  const clearWorkspace = useCallback(() => {
    sessionStorage.removeItem(NAME_STORAGE_KEY);
    notify();
  }, []);

  /// Leaves the workspace *and* returns to the tenant dashboard — what the sidebar's "Back"
  /// means, as opposed to merely forgetting which workspace was open.
  const leaveWorkspace = useCallback(() => {
    sessionStorage.removeItem(NAME_STORAGE_KEY);
    notify();
    navigate("/dashboard");
  }, [navigate]);

  const wsPath = useCallback((subpath: string) => `/ws/${wsId}/${subpath}`, [wsId]);

  return {
    workspaceId,
    shortId,
    workspaceName,
    selectWorkspace,
    clearWorkspace,
    leaveWorkspace,
    wsPath,
  };
}
