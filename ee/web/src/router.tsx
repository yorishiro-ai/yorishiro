import { useEffect } from "react";
import { createBrowserRouter, Navigate, Outlet, useParams } from "react-router-dom";
import { AppShell } from "./components/layout/AppShell";
import { useAuth } from "./hooks/useAuth";
import { useWorkspace } from "./hooks/useWorkspace";
import { ApiKeysPage } from "./pages/ApiKeysPage";
import { DashboardPage } from "./pages/DashboardPage";
import { EntitiesPage } from "./pages/EntitiesPage";
import { MarketplacePage } from "./pages/MarketplacePage";
import { MarketplaceDetailPage } from "./pages/MarketplaceDetailPage";
import { EntityCreatePage } from "./pages/EntityCreatePage";
import { EntityDetailPage } from "./pages/EntityDetailPage";
import { GraphPage } from "./pages/GraphPage";
import { LoginPage } from "./pages/LoginPage";
import { MembersPage } from "./pages/MembersPage";
import { SearchPage } from "./pages/SearchPage";
import { SchemaDetailPage } from "./pages/SchemaDetailPage";
import { SchemasPage } from "./pages/SchemasPage";
import { SetupPage } from "./pages/SetupPage";
import { TemplateDetailPage } from "./pages/TemplateDetailPage";
import { SignupPage } from "./pages/SignupPage";
import { WsDashboardPage } from "./pages/WsDashboardPage";
import { WorkspacesPage } from "./pages/WorkspacesPage";
import { WsSchemaPage } from "./pages/WsSchemaPage";

function RequireAuth() {
  const { isAuthenticated } = useAuth();
  if (!isAuthenticated) return <Navigate to="/login" replace />;
  return <AuthenticatedLayout />;
}

function AuthenticatedLayout() {
  const { email, logout } = useAuth();
  return <AppShell email={email} onLogout={logout} />;
}

function PublicOnly() {
  const { isAuthenticated } = useAuth();
  if (isAuthenticated) return <Navigate to="/dashboard" replace />;
  return <Outlet />;
}

function TenantScope() {
  const { clearWorkspace } = useWorkspace();
  useEffect(() => {
    clearWorkspace();
  }, [clearWorkspace]);
  return <Outlet />;
}

function RequireWorkspace() {
  const { wsId } = useParams();
  if (!wsId) return <Navigate to="/dashboard" replace />;
  return <Outlet />;
}

export const router = createBrowserRouter([
  {
    element: <PublicOnly />,
    children: [
      { path: "/login", element: <LoginPage /> },
      { path: "/signup", element: <SignupPage /> },
      { path: "/setup", element: <SetupPage /> },
    ],
  },
  {
    element: <RequireAuth />,
    children: [
      {
        element: <TenantScope />,
        children: [
          { path: "/dashboard", element: <DashboardPage /> },
          { path: "/members", element: <MembersPage /> },
          { path: "/schemas", element: <SchemasPage /> },
          { path: "/marketplace", element: <MarketplacePage /> },
          { path: "/marketplace/:templateId", element: <MarketplaceDetailPage /> },
          // The literal path comes first. `/schemas/:schemaId` also matches
          // `/schemas/templates/worldbuilding` with schemaId="templates", and the detail page
          // fails on an id that is not a UUID -- dropping the user on the dashboard instead.
          { path: "/schemas/templates/:id", element: <TemplateDetailPage /> },
          { path: "/schemas/:schemaId", element: <SchemaDetailPage /> },
          { path: "/workspaces", element: <WorkspacesPage /> },
          { path: "/api-keys", element: <ApiKeysPage /> },
        ],
      },
      {
        element: <RequireWorkspace />,
        children: [
          { path: "/ws/:wsId/dashboard", element: <WsDashboardPage /> },
          { path: "/ws/:wsId/schema", element: <WsSchemaPage /> },
          { path: "/ws/:wsId/schema/io", element: <WsSchemaPage /> },
          { path: "/ws/:wsId/entities", element: <EntitiesPage /> },
          { path: "/ws/:wsId/entities/new", element: <EntityCreatePage /> },
          { path: "/ws/:wsId/entities/:id", element: <EntityDetailPage /> },
          { path: "/ws/:wsId/graph", element: <GraphPage /> },
          { path: "/ws/:wsId/search", element: <SearchPage /> },
          { path: "/ws/:wsId/import-export", element: <Navigate to="../schema/io" replace /> },
        ],
      },
    ],
  },
  { path: "/templates", element: <Navigate to="/schemas" replace /> },
  { path: "/", element: <Navigate to="/dashboard" replace /> },
  { path: "*", element: <Navigate to="/dashboard" replace /> },
]);
