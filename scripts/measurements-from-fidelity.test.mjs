#!/usr/bin/env node
// Guard for scripts/measurements-from-fidelity.mjs.
//
// The reader turns the fidelity gate's per-fixture output into an evidence
// artifact and a measurement artifact. Three properties are worth pinning,
// and the second is the one that protects the trend from being poisoned:
//
//   1. Aggregates only, and correct: worst page per fixture, never per-page
//      rows (that is the difference between a trend file and a warehouse).
//   2. INCONCLUSIVE is carried, not silently converted into a number. When
//      the runner has no FOGRA39 profile the two sides sit in different
//      colour spaces and the per-page figures compare nothing; letting those
//      into the series would poison it permanently.
//   3. A fixture that produced NO gate is REPORTED, never skipped past. A
//      reader that quietly drops a broken fixture turns a red run green.
//
// Run: node scripts/measurements-from-fidelity.test.mjs

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, "measurements-from-fidelity.mjs");

/** Build a fixture directory the way diff.sh would leave one. */
function fixture(root, name, { pages, gate, noProfile = false }) {
  const dir = join(root, name);
  mkdirSync(dir, { recursive: true });
  if (pages) writeFileSync(join(dir, "report.json"), JSON.stringify(pages));
  if (gate) writeFileSync(join(dir, "gate.json"), JSON.stringify(gate));
  if (noProfile) writeFileSync(join(dir, ".no-cmyk-profile"), "");
  return dir;
}

function run(inDir) {
  const outDir = mkdtempSync(join(tmpdir(), "fid-out-"));
  execFileSync(
    process.execPath,
    [
      SCRIPT,
      "--in", inDir,
      "--out-dir", outDir,
      "--commit", "deadbeef",
      "--branch", "main",
      "--os", "ubuntu-latest",
      "--finished-at", "2026-08-22T18:00:00Z",
    ],
    { stdio: "pipe" },
  );
  return {
    measurements: JSON.parse(
      readFileSync(join(outDir, "paged-measurements-fidelity.json"), "utf8"),
    ),
    results: JSON.parse(
      readFileSync(join(outDir, "paged-results-fidelity.json"), "utf8"),
    ),
  };
}

const PASSING_PAGES = [
  { page: 1, mean_de: 0.05, p99_de: 0.0, ssim: 0.997 },
  { page: 2, mean_de: 0.11, p99_de: 0.4, ssim: 0.991 },
  { page: 3, mean_de: 0.08, p99_de: 0.2, ssim: 0.994 },
];
const PASSING_GATE = {
  fixture: "geometry",
  pages_checked: 3,
  pages_total: 3,
  passed: true,
  thresholds: { max_mean_de: 0.13, max_p99_de: 0.5, min_ssim: 0.99 },
  failures: [],
};

test("aggregates are the WORST page, and only aggregates are emitted", () => {
  const root = mkdtempSync(join(tmpdir(), "fid-in-"));
  fixture(root, "geometry", { pages: PASSING_PAGES, gate: PASSING_GATE });
  const { measurements } = run(root);

  const by = Object.fromEntries(
    measurements.measurements.map((m) => [m.metric, m]),
  );
  assert.equal(by["fidelity.worst_mean_de"].value, 0.11, "max of the means");
  assert.equal(by["fidelity.worst_p99_de"].value, 0.4, "max of the p99s");
  assert.equal(by["fidelity.worst_ssim"].value, 0.991, "MIN ssim — lower is worse");
  assert.equal(by["fidelity.pages_checked"].value, 3);

  // Four aggregates for one fixture — never one row per page.
  assert.equal(measurements.measurements.length, 4);
  assert.equal(by["fidelity.worst_mean_de"].subject, "geometry");
});

test("each measurement carries the budget the gate actually enforced", () => {
  const root = mkdtempSync(join(tmpdir(), "fid-in-"));
  fixture(root, "geometry", { pages: PASSING_PAGES, gate: PASSING_GATE });
  const { measurements } = run(root);
  const by = Object.fromEntries(
    measurements.measurements.map((m) => [m.metric, m]),
  );
  assert.equal(by["fidelity.worst_mean_de"].budget, 0.13);
  assert.equal(by["fidelity.worst_ssim"].budget, 0.99);
});

test("the trend key carries the OS — a matrix would otherwise alternate", () => {
  const root = mkdtempSync(join(tmpdir(), "fid-in-"));
  fixture(root, "geometry", { pages: PASSING_PAGES, gate: PASSING_GATE });
  const { measurements } = run(root);
  assert.equal(measurements.environment.os, "ubuntu-latest");
});

test("a passing gate produces a PASSED evidence row, contract v1 shaped", () => {
  const root = mkdtempSync(join(tmpdir(), "fid-in-"));
  fixture(root, "geometry", { pages: PASSING_PAGES, gate: PASSING_GATE });
  const { results } = run(root);
  assert.equal(results.contract, 1);
  assert.equal(results.source, "core");
  assert.equal(results.suite, "fidelity");
  assert.deepEqual(results.results[0], {
    test_id: "fidelity/geometry",
    features: ["test-corpus.fidelity-gate"],
    status: "passed",
  });
});

test("INCONCLUSIVE is carried through and never becomes a plain number", () => {
  const root = mkdtempSync(join(tmpdir(), "fid-in-"));
  fixture(root, "tables", {
    // The colour-space giveaway: a near-uniform p99 of ~4.16 on every page.
    pages: [
      { page: 1, mean_de: 3.9, p99_de: 4.16, ssim: 0.72 },
      { page: 2, mean_de: 4.0, p99_de: 4.16, ssim: 0.71 },
    ],
    gate: {
      fixture: "tables",
      pages_checked: 2,
      passed: false,
      thresholds: { max_mean_de: 1.1, max_p99_de: 2.5, min_ssim: 0.96 },
      failures: [{ page: 1, violations: ["meanΔE 3.900 > 1.100"] }],
    },
    noProfile: true,
  });
  const { measurements, results } = run(root);

  // Every measurement is flagged, and says WHY, so a consumer can exclude it.
  assert.ok(measurements.measurements.length > 0);
  for (const m of measurements.measurements) {
    assert.equal(m.status, "inconclusive");
    assert.match(m.reason, /FOGRA39|excluded from the trend/);
  }

  // The evidence row is SKIPPED — emphatically not "failed". A colour-space
  // mismatch is not a regression, and reporting it as one is the exact
  // dishonesty the gate's own three-state exit was added to prevent.
  assert.equal(results.results[0].status, "skipped");
  assert.match(results.results[0].note, /inconclusive/);
});

test("a real regression is reported as failed, with the worst page named", () => {
  const root = mkdtempSync(join(tmpdir(), "fid-in-"));
  fixture(root, "text", {
    pages: [{ page: 4, mean_de: 0.9, p99_de: 1.2, ssim: 0.95 }],
    gate: {
      fixture: "text",
      pages_checked: 1,
      passed: false,
      thresholds: { max_mean_de: 0.65, max_p99_de: 1.0, min_ssim: 0.97 },
      failures: [
        { page: 4, mean_de: 0.9, violations: ["meanΔE 0.900 > 0.650"] },
      ],
    },
  });
  const { results, measurements } = run(root);
  assert.equal(results.results[0].status, "failed");
  assert.match(results.results[0].note, /page 4/);
  assert.match(results.results[0].note, /0\.900 > 0\.650/);
  // A regression is still a MEASUREMENT — it is conclusive, just bad.
  assert.equal(measurements.measurements[0].status, "ok");
});

test("an ungated fixture is skipped with its reason, not invented", () => {
  const root = mkdtempSync(join(tmpdir(), "fid-in-"));
  fixture(root, "newthing", {
    gate: { fixture: "newthing", skipped: true, reason: "not in manifest" },
  });
  const { results, measurements } = run(root);
  assert.equal(results.results[0].status, "skipped");
  assert.match(results.results[0].note, /not in manifest/);
  assert.equal(measurements.measurements.length, 0);
});

test("a fixture that produced NO gate is reported, never silently dropped", () => {
  const root = mkdtempSync(join(tmpdir(), "fid-in-"));
  fixture(root, "geometry", { pages: PASSING_PAGES, gate: PASSING_GATE });
  fixture(root, "broken", { pages: PASSING_PAGES }); // render died: no gate.json
  const { results } = run(root);

  assert.equal(results.results.length, 2, "the broken fixture must still appear");
  const broken = results.results.find((r) => r.test_id === "fidelity/broken");
  assert.equal(broken.status, "failed");
  assert.match(broken.note, /no gate\.json/);
});
