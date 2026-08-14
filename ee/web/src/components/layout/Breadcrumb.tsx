import { useLocation, NavLink } from "react-router-dom";
import { ChevronRight } from "lucide-react";
import { useWorkspace } from "@/hooks/useWorkspace";

interface BreadcrumbItem {
  label: string;
  to: string | null;
}

function buildBreadcrumbItems(pathname: string, workspaceLabel: string): BreadcrumbItem[] {
  const segments = pathname.split("/").filter(Boolean);

  if (segments.length === 0) {
    return [];
  }

  const [root, ...rest] = segments;

  switch (root) {
    case "dashboard":
      return [{ label: "Dashboard", to: null }];
    case "members":
      return [{ label: "Members", to: null }];
    case "schemas": {
      if (rest.length === 0) {
        return [{ label: "Schemas", to: null }];
      }
      const [name] = rest;
      return [
        { label: "Schemas", to: "/schemas" },
        { label: name, to: null },
      ];
    }
    case "api-keys":
      return [{ label: "API Keys", to: null }];
    case "ws": {
      // /ws/:wsId/<section>/[id]
      const [wsId, section, id] = rest;
      const wsBase = `/ws/${wsId}`;
      const base: BreadcrumbItem[] = [
        { label: `Workspace: ${workspaceLabel}`, to: `${wsBase}/dashboard` },
      ];

      switch (section) {
        case "schema": {
          if (id === "io") {
            return [
              base[0],
              { label: "Schema", to: `${wsBase}/schema` },
              { label: "Import / Export", to: null },
            ];
          }
          return [...base.slice(0, 1), { label: "Schema", to: null }];
        }
        case "entities": {
          if (id === "new") {
            return [
              base[0],
              { label: "Entities", to: `${wsBase}/entities` },
              { label: "New Entity", to: null },
            ];
          }
          if (id) {
            return [
              base[0],
              { label: "Entities", to: `${wsBase}/entities` },
              { label: id.slice(0, 8), to: null },
            ];
          }
          return [...base.slice(0, 1), { label: "Entities", to: null }];
        }
        case "graph":
          return [...base.slice(0, 1), { label: "Graph", to: null }];
        case "search":
          return [...base.slice(0, 1), { label: "Search", to: null }];
        default:
          return base.slice(0, 1);
      }
    }
    default:
      return [];
  }
}

export function Breadcrumb() {
  const location = useLocation();
  const { workspaceName } = useWorkspace();

  const workspaceLabel = workspaceName ?? "Workspace";
  const items = buildBreadcrumbItems(location.pathname, workspaceLabel);

  if (items.length === 0) {
    return null;
  }

  return (
    <nav
      aria-label="Breadcrumb"
      className="flex items-center gap-1.5 text-sm text-muted-foreground"
    >
      {items.map((item, index) => {
        const isLast = index === items.length - 1;
        return (
          <span key={`${item.label}-${index}`} className="flex items-center gap-1.5">
            {index > 0 && <ChevronRight className="h-3.5 w-3.5 shrink-0" />}
            {isLast || !item.to ? (
              <span className={isLast ? "font-medium text-foreground" : undefined}>
                {item.label}
              </span>
            ) : (
              <NavLink to={item.to} className="transition-colors hover:text-foreground">
                {item.label}
              </NavLink>
            )}
          </span>
        );
      })}
    </nav>
  );
}
