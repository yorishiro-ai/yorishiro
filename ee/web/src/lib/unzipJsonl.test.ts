/**
 * ZIP import feeds each extracted member to the same `/api/import.jsonl` endpoint the
 * single-file path uses. Anything that is not a `.jsonl` payload must be filtered out here,
 * because the endpoint would reject it as malformed content the user never chose to import --
 * and real archives are full of such members (`__MACOSX/`, `.DS_Store`, directory entries).
 */
import { describe, expect, it } from "vitest";
import { zipSync, strToU8 } from "fflate";
import { unzipJsonl } from "./unzipJsonl";

function makeZip(files: Record<string, string>): Uint8Array {
  return zipSync(Object.fromEntries(Object.entries(files).map(([k, v]) => [k, strToU8(v)])));
}

describe("unzipJsonl", () => {
  it("extracts jsonl members with their contents", () => {
    const zip = makeZip({ "entities.jsonl": '{"a":1}\n{"a":2}\n' });
    expect(unzipJsonl(zip)).toEqual([{ name: "entities.jsonl", content: '{"a":1}\n{"a":2}\n' }]);
  });

  it("returns members in name order so schemas can be applied before entities", () => {
    const zip = makeZip({
      "02-entities.jsonl": '{"e":1}\n',
      "01-schemas.jsonl": '{"s":1}\n',
    });
    expect(unzipJsonl(zip).map((f) => f.name)).toEqual(["01-schemas.jsonl", "02-entities.jsonl"]);
  });

  it("skips non-jsonl members", () => {
    const zip = makeZip({ "readme.txt": "hello", "data.jsonl": '{"a":1}\n' });
    expect(unzipJsonl(zip).map((f) => f.name)).toEqual(["data.jsonl"]);
  });

  it("skips macOS resource forks and dotfiles", () => {
    const zip = makeZip({
      "__MACOSX/data.jsonl": "junk",
      ".hidden.jsonl": "junk",
      "data.jsonl": '{"a":1}\n',
    });
    expect(unzipJsonl(zip).map((f) => f.name)).toEqual(["data.jsonl"]);
  });

  it("returns nothing for an archive with no jsonl members", () => {
    expect(unzipJsonl(makeZip({ "notes.md": "hi" }))).toEqual([]);
  });
});
