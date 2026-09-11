import { useEffect, useState } from "react";

interface EntityRecord {
  id: string;
  entity_type: string;
  data: unknown;
  schema_version: string;
  created_at: string;
  updated_at: string;
}

const API_BASE = import.meta.env.VITE_API_BASE ?? "";

function EntityRow({ entity }: { entity: EntityRecord }) {
  return (
    <tr>
      <td>{entity.id}</td>
      <td>{entity.entity_type}</td>
      <td>{entity.schema_version}</td>
      <td>{entity.created_at}</td>
      <td>{entity.updated_at}</td>
    </tr>
  );
}

function EntityList() {
  const [entities, setEntities] = useState<EntityRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetch(`${API_BASE}/api/entities`)
      .then((res) => {
        if (!res.ok) throw new Error(`HTTP ${res.status}`);
        return res.json();
      })
      .then((data: EntityRecord[]) => {
        if (!cancelled) {
          setEntities(data);
          setLoading(false);
        }
      })
      .catch((err: Error) => {
        if (!cancelled) {
          setError(err.message);
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (loading) return <p>Loading entities…</p>;
  if (error) return <p style={{ color: "red" }}>Failed to load: {error}</p>;
  if (entities.length === 0) return <p>No entities found.</p>;

  return (
    <table>
      <thead>
        <tr>
          <th>ID</th>
          <th>Type</th>
          <th>Schema</th>
          <th>Created</th>
          <th>Updated</th>
        </tr>
      </thead>
      <tbody>
        {entities.map((e) => (
          <EntityRow key={e.id} entity={e} />
        ))}
      </tbody>
    </table>
  );
}

export default function App() {
  return (
    <div style={{ padding: "1rem", fontFamily: "sans-serif" }}>
      <h1>Yorishiro — Entities</h1>
      <EntityList />
    </div>
  );
}
