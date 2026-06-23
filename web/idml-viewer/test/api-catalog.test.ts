/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

// Keeps api-catalog.json honest: every public export of src/index.ts must appear
// in the catalog (as a member name, or as the object prefix of a member — e.g.
// the `Viewer` interface is documented via `Viewer.load`, `Viewer.zoom`, …). The
// docs site renders the SDK reference from this catalog, so a new export that
// isn't documented here fails the build instead of silently shipping undocumented.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { describe, it, expect } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");

function exportedNames(indexTs: string): string[] {
  const names = new Set<string>();
  // Match each `export { ... }` / `export type { ... }` block and pull identifiers.
  for (const m of indexTs.matchAll(/export\s+(?:type\s+)?\{([^}]*)\}/g)) {
    for (const raw of m[1].split(",")) {
      const id = raw.trim().replace(/^type\s+/, "").split(/\s+as\s+/)[0].trim();
      if (id) names.add(id);
    }
  }
  return [...names];
}

describe("api-catalog.json", () => {
  const indexTs = readFileSync(join(root, "src", "index.ts"), "utf8");
  const catalog = JSON.parse(readFileSync(join(root, "api-catalog.json"), "utf8")) as {
    groups: { members: { name: string }[] }[];
  };
  const memberNames = catalog.groups.flatMap((g) => g.members.map((m) => m.name));
  const exports = exportedNames(indexTs);

  it("documents every export of src/index.ts", () => {
    const covered = (id: string) => memberNames.some((n) => n === id || n.startsWith(`${id}.`));
    const missing = exports.filter((id) => !covered(id));
    expect(missing, `undocumented exports — add them to api-catalog.json: ${missing.join(", ")}`).toEqual([]);
  });

  it("has well-formed members (name + signature + summary)", () => {
    for (const g of catalog.groups) {
      for (const m of g.members as { name: string; signature?: string; summary?: string }[]) {
        expect(m.name, "member missing name").toBeTruthy();
        expect(m.signature, `member ${m.name} missing signature`).toBeTruthy();
        expect(m.summary, `member ${m.name} missing summary`).toBeTruthy();
      }
    }
  });
});
