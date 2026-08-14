import { Outlet } from "react-router-dom";
import { Sidebar } from "@/components/layout/Sidebar";
import { Breadcrumb } from "./Breadcrumb";
import { useTheme } from "@/hooks/useTheme";

interface AppShellProps {
  email: string | null;
  onLogout: () => void;
}

export function AppShell({ email, onLogout }: AppShellProps) {
  const { isDark, toggle } = useTheme();

  return (
    <div className="flex min-h-screen">
      <Sidebar email={email} onLogout={onLogout} isDark={isDark} onToggleTheme={toggle} />
      <main className="flex-1 overflow-y-auto">
        <div className="sticky top-0 px-8 pt-6 pb-2">
          <Breadcrumb />
        </div>
        <div className="px-8 pb-8">
          <Outlet />
        </div>
      </main>
    </div>
  );
}
