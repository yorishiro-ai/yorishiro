import { useCallback, useSyncExternalStore } from "react";
import { getApiKey, getSessionEmail, clearSession, setSession } from "@/lib/api";
import { createExternalStore } from "@/lib/externalStore";

const { subscribe, notify } = createExternalStore();

function getSnapshot() {
  return getApiKey();
}

export function useAuth() {
  const apiKey = useSyncExternalStore(subscribe, getSnapshot);
  const email = getSessionEmail();

  const loginSession = useCallback((key: string, userEmail: string, workspaceId?: string) => {
    setSession(key, userEmail, workspaceId);
    notify();
  }, []);

  const logout = useCallback(() => {
    clearSession();
    notify();
  }, []);

  return {
    isAuthenticated: apiKey !== null,
    apiKey,
    email,
    loginSession,
    logout,
  };
}
