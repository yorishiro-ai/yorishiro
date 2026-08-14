import { NavLink, useNavigate } from "react-router-dom";
import {
  LayoutDashboard,
  Users,
  Database,
  KeyRound,
  FolderOpen,
  FileText,
  Network,
  Search,
  LogOut,
  ArrowLeft,
  Sun,
  Moon,
  Store,
} from "lucide-react";
import { useWorkspace } from "@/hooks/useWorkspace";

interface SidebarProps {
  email: string | null;
  onLogout: () => void;
  isDark: boolean;
  onToggleTheme: () => void;
}

const TENANT_NAV = [
  { label: "Dashboard", to: "/dashboard", icon: LayoutDashboard },
  { label: "Workspaces", to: "/workspaces", icon: FolderOpen },
  { label: "Members", to: "/members", icon: Users },
  { label: "Schemas", to: "/schemas", icon: Database },
  { label: "Marketplace", to: "/marketplace", icon: Store },
  { label: "API Keys", to: "/api-keys", icon: KeyRound },
];

function buildWsNav(wsId: string) {
  return [
    { label: "Dashboard", to: `/ws/${wsId}/dashboard`, icon: LayoutDashboard },
    { label: "Schema", to: `/ws/${wsId}/schema`, icon: Database },
    { label: "Entities", to: `/ws/${wsId}/entities`, icon: FileText },
    { label: "Graph", to: `/ws/${wsId}/graph`, icon: Network },
    { label: "Search", to: `/ws/${wsId}/search`, icon: Search },
  ];
}

function NavItem({
  label,
  to,
  icon: Icon,
}: {
  label: string;
  to: string;
  icon: typeof LayoutDashboard;
}) {
  return (
    <li>
      <NavLink
        to={to}
        className="flex items-center gap-3 rounded-lg px-3 py-2 text-[13px] font-medium transition-colors"
        style={({ isActive }) => ({
          backgroundColor: isActive ? "var(--color-sidebar-active)" : "transparent",
          color: isActive ? "#fff" : "var(--color-sidebar-foreground)",
        })}
      >
        <Icon className="h-[18px] w-[18px] shrink-0" />
        <span className="truncate">{label}</span>
      </NavLink>
    </li>
  );
}

export function Sidebar({ email, onLogout, isDark, onToggleTheme }: SidebarProps) {
  const navigate = useNavigate();
  const { workspaceId, shortId, workspaceName } = useWorkspace();

  const inWorkspaceMode = workspaceId !== null;
  const wsNav = workspaceId ? buildWsNav(workspaceId) : [];
  const wsLabel = workspaceName ?? (shortId ? `${shortId}…` : "Workspace");

  function handleBack() {
    navigate("/dashboard");
  }

  return (
    <aside
      className="flex h-screen w-60 flex-col shrink-0"
      style={{
        backgroundColor: "var(--color-sidebar)",
        borderRight: "1px solid var(--color-sidebar-border)",
      }}
    >
      {/* Header */}
      <div
        className="flex items-center h-14 px-4 justify-between"
        style={{ borderBottom: "1px solid var(--color-sidebar-border)" }}
      >
        <span className="text-sm font-bold tracking-widest uppercase" style={{ color: "#e4e4e7" }}>
          Yorishiro
        </span>
      </div>

      {/* Nav */}
      <nav className="flex-1 overflow-y-auto px-2 py-3">
        {inWorkspaceMode ? (
          <>
            <button
              type="button"
              onClick={handleBack}
              className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-[13px] font-medium transition-colors"
              style={{ color: "var(--color-sidebar-foreground)" }}
            >
              <ArrowLeft className="h-4 w-4 shrink-0" />
              <span>Back</span>
            </button>

            <div
              className="px-3 pb-3 pt-2 text-sm font-semibold truncate"
              style={{ color: "#e4e4e7" }}
              title={workspaceName ?? workspaceId ?? undefined}
            >
              {wsLabel}
            </div>

            <ul className="flex flex-col gap-0.5">
              {wsNav.map((item) => (
                <NavItem key={item.to} {...item} />
              ))}
            </ul>
          </>
        ) : (
          <>
            <div
              className="px-3 pb-1 text-[11px] font-semibold uppercase tracking-wider"
              style={{ color: "var(--color-sidebar-foreground)", opacity: 0.5 }}
            >
              Organization
            </div>
            <ul className="flex flex-col gap-0.5">
              {TENANT_NAV.map((item) => (
                <NavItem key={item.to} {...item} />
              ))}
            </ul>
          </>
        )}
      </nav>

      {/* Footer */}
      <div
        className="flex items-center gap-1 px-3 py-2"
        style={{ borderTop: "1px solid var(--color-sidebar-border)" }}
      >
        {email && (
          <div
            className="flex-1 truncate text-[11px]"
            style={{ color: "var(--color-sidebar-foreground)", opacity: 0.5 }}
            title={email}
          >
            {email}
          </div>
        )}
        <button
          type="button"
          onClick={onToggleTheme}
          className="rounded-md p-1.5 transition-colors"
          style={{ color: "var(--color-sidebar-foreground)" }}
          title={isDark ? "Light mode" : "Dark mode"}
          // Icon-only: `title` alone leaves the button unnamed for a screen reader,
          // which announces it as just "button".
          aria-label={isDark ? "Switch to light mode" : "Switch to dark mode"}
        >
          {isDark ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
        </button>
        <button
          type="button"
          onClick={onLogout}
          className="rounded-md p-1.5 transition-colors"
          style={{ color: "var(--color-sidebar-foreground)" }}
          title="Sign Out"
          aria-label="Sign out"
        >
          <LogOut className="h-4 w-4" />
        </button>
      </div>
    </aside>
  );
}
