import { unzipSync } from "fflate";

/** One `.jsonl` member of an uploaded archive. */
export interface ZipEntry {
  name: string;
  content: string;
}

/**
 * Extracts the `.jsonl` members of a ZIP archive, in name order.
 *
 * Only `.jsonl` files are returned. Archives commonly carry directory entries and editor/OS
 * cruft (`__MACOSX/`, `.DS_Store`), and feeding any of those to the import endpoint would fail
 * on content that was never meant to be imported.
 *
 * Ordering is by name so that a multi-file import is reproducible: the endpoint applies each
 * file in turn, and schemas must land before the entities referencing them. Callers are
 * expected to name files accordingly (`01-schemas.jsonl`, `02-entities.jsonl`).
 */
export function unzipJsonl(bytes: Uint8Array): ZipEntry[] {
  const files = unzipSync(bytes);
  const decoder = new TextDecoder();

  return Object.entries(files)
    .filter(([name, data]) => {
      if (!name.toLowerCase().endsWith(".jsonl")) return false;
      // Directory entries decode to zero bytes; a real .jsonl never does.
      if (data.length === 0) return false;
      const base = name.split("/").pop() ?? name;
      return !name.startsWith("__MACOSX/") && !base.startsWith(".");
    })
    .toSorted(([a], [b]) => a.localeCompare(b))
    .map(([name, data]) => ({ name, content: decoder.decode(data) }));
}
