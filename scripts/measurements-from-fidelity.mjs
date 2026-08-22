#!/usr/bin/env node
/**
 * Turn the fidelity gate's output into things that OUTLIVE the run.
 *
 * WHY THIS EXISTS
 *
 *   `corpus/generated/diff.sh` computes per-page ΔE2000 and SSIM against
 *   InDesign's own exported PDFs — the single most valuable measured signal
 *   in this project — writes `report.json` + `gate.json` per fixture, and
 *   then the workflow uploads them ONLY `if: failure()`. Every passing run's
 *   numbers are discarded, so nobody can answer "is fidelity drifting?"
 *   without re-running history by hand.
 *
 *   Worse, `gate.json` already carries a real, reproducible, per-fixture
 *   ASSERTION (`passed`), and `fidelity.yml` publishes no results artifact at
 *   all — so a full fidelity gate contributes ZERO rows to the state
 *   dashboard. That is not a missing measurement category; it is a missing
 *   evidence lane.
 *
 * SO THIS EMITS TWO FILES, AND THE SPLIT IS THE WHOLE POINT
 *
 *   1. `paged-results-fidelity.json` — contract v1 EVIDENCE. One row per
 *      fixture, because one fixture is one thing asserted. This needs no
 *      schema change anywhere: `test_id` + `status` + `note` already exist.
 *
 *   2. `paged-measurements-fidelity.json` — the NUMBERS, which are not
 *      evidence and must never be mistaken for it. A ΔE of 0.108 asserts
 *      nothing about whether a feature works; the gate's pass/fail verdict is
 *      the assertion, and it travels in file 1.
 *
 * WHAT IT DELIBERATELY DOES NOT DO
 *
 *   · It does not modify `diff.sh`. That script's 0/1/3 exit semantics are
 *     load-bearing and freshly reasoned; a separate reader leaves them alone.
 *   · It emits AGGREGATES ONLY — worst page per fixture, not all ~96 pages.
 *     Per-page series across fixtures × OSes × every run is the thing that
 *     turns a trend file into a metrics warehouse. The detail stays in the
 *     CI artifact you download to triage.
 *   · It never invents a number. A fixture with no `gate.json` is REPORTED
 *     as unreadable, not silently dropped.
 *
 * INCONCLUSIVE IS A FIRST-CLASS OUTCOME
 *
 *   The reference PDFs are Coated FOGRA39 and no runner has that profile, so
 *   the gate rasterises them in a different colour space and the per-page
 *   numbers compare nothing (the giveaway is a near-uniform p99 of ~4.16).
 *   `diff.sh` already exits 3 and drops a `.no-cmyk-profile` marker for this.
 *   Such a measurement is recorded with `status: "inconclusive"` and MUST be
 *   excluded from any trend — letting it into the series would poison it
 *   permanently — while the evidence row becomes `skipped` with the reason.
 *   Recorded and flagged, never silently swallowed.
 *
 * Usage:
 *   node scripts/measurements-from-fidelity.mjs --in /tmp/idml-generated-diff \
 *     [--out-dir .] [--commit SHA] [--branch main] [--run-id N] [--os ubuntu-latest]
 */

import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const FEATURE = "test-corpus.fidelity-gate";
const NO_PROFILE_MARKER = ".no-cmyk-profile";

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i !== -1 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
}

const inDir = resolve(arg("in", "/tmp/idml-generated-diff"));
const outDir = resolve(arg("out-dir", "."));
const commit = arg("commit", process.env.GITHUB_SHA ?? "unknown");
const branch = arg("branch", process.env.GITHUB_REF_NAME ?? "main");
const runId = arg("run-id", process.env.GITHUB_RUN_ID);
const osName = arg("os", process.env.RUNNER_OS ?? process.platform);
const finishedAt = arg("finished-at", new Date().toISOString());

if (!existsSync(inDir)) {
  console.error(`[fidelity-measurements] input dir not found: ${inDir}`);
  process.exit(2);
}

/** Every immediate subdirectory that looks like a fixture output. */
const fixtures = readdirSync(inDir, { withFileTypes: true })
  .filter((e) => e.isDirectory())
  .map((e) => e.name)
  .sort();

const measurements = [];
const results = [];
const unreadable = [];

for (const fixture of fixtures) {
  const dir = join(inDir, fixture);
  const gatePath = join(dir, "gate.json");
  if (!existsSync(gatePath)) {
    // Honest: a fixture that produced no gate is a fact worth reporting, not
    // an absence to skip past. It usually means the render step died.
    unreadable.push(fixture);
    results.push({
      test_id: `fidelity/${fixture}`,
      features: [FEATURE],
      status: "failed",
      note: "no gate.json — the fixture produced no gate output at all",
    });
    continue;
  }

  let gate;
  try {
    gate = JSON.parse(readFileSync(gatePath, "utf8"));
  } catch (err) {
    unreadable.push(fixture);
    results.push({
      test_id: `fidelity/${fixture}`,
      features: [FEATURE],
      status: "failed",
      note: `gate.json unreadable: ${String(err)}`,
    });
    continue;
  }

  // A fixture absent from fidelity-thresholds.json is deliberately ungated.
  if (gate.skipped) {
    results.push({
      test_id: `fidelity/${fixture}`,
      features: [FEATURE],
      status: "skipped",
      note: String(gate.reason ?? "not in fidelity-thresholds.json"),
    });
    continue;
  }

  const inconclusive = existsSync(join(dir, NO_PROFILE_MARKER));

  // ── the evidence row ────────────────────────────────────────────────
  results.push({
    test_id: `fidelity/${fixture}`,
    features: [FEATURE],
    status: inconclusive ? "skipped" : gate.passed ? "passed" : "failed",
    note: inconclusive
      ? "inconclusive: no FOGRA39 profile on this runner, so the reference and " +
        "candidate sit in different colour spaces and the gate measured nothing"
      : gate.passed
        ? undefined
        : summariseFailures(gate),
  });

  // ── the measurements ────────────────────────────────────────────────
  const pages = readPages(dir);
  if (pages.length === 0) continue;

  const gated = pages.filter(
    (p) => typeof p.page === "number" && p.page <= (gate.pages_checked ?? Infinity),
  );
  const scope = gated.length > 0 ? gated : pages;
  const th = gate.thresholds ?? {};
  const status = inconclusive ? "inconclusive" : "ok";
  const reason = inconclusive
    ? "no FOGRA39 profile on this runner — excluded from the trend"
    : undefined;

  push("fidelity.worst_mean_de", max(scope, "mean_de"), "de2000", th.max_mean_de);
  push("fidelity.worst_p99_de", max(scope, "p99_de"), "de2000", th.max_p99_de);
  push("fidelity.worst_ssim", min(scope, "ssim"), "ssim", th.min_ssim);
  push("fidelity.pages_checked", scope.length, "count", undefined);

  function push(metric, value, unit, budget) {
    if (value === undefined || !Number.isFinite(value)) return;
    const m = { metric, subject: fixture, value: round(value), unit, status };
    if (budget !== undefined) m.budget = budget;
    if (reason) m.reason = reason;
    measurements.push(m);
  }
}

function readPages(dir) {
  const p = join(dir, "report.json");
  if (!existsSync(p)) return [];
  try {
    const parsed = JSON.parse(readFileSync(p, "utf8"));
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function max(rows, key) {
  const vals = rows.map((r) => r[key]).filter((v) => typeof v === "number");
  return vals.length ? Math.max(...vals) : undefined;
}
function min(rows, key) {
  const vals = rows.map((r) => r[key]).filter((v) => typeof v === "number");
  return vals.length ? Math.min(...vals) : undefined;
}
function round(n) {
  return Math.round(n * 10000) / 10000;
}

function summariseFailures(gate) {
  const n = Array.isArray(gate.failures) ? gate.failures.length : 0;
  if (n === 0) return "gate reported a failure with no page detail";
  const first = gate.failures[0];
  const violations = Array.isArray(first?.violations)
    ? first.violations.join("; ")
    : "";
  return `${n} page(s) outside tolerance — worst: page ${first?.page} ${violations}`;
}

const measurementsDoc = {
  contract: 1,
  source: "core",
  lane: "fidelity",
  commit,
  branch,
  ...(runId ? { run_id: String(runId) } : {}),
  finished_at: finishedAt,
  // The trend key is (metric, subject, os). Without the OS dimension a
  // ubuntu/macos matrix alternates values and reads as a regression every
  // other run.
  environment: { os: String(osName), arch: process.arch },
  measurements,
};

const resultsDoc = {
  contract: 1,
  source: "core",
  suite: "fidelity",
  commit,
  branch,
  ...(runId ? { run_id: String(runId) } : {}),
  finished_at: finishedAt,
  results,
};

const mPath = join(outDir, "paged-measurements-fidelity.json");
const rPath = join(outDir, "paged-results-fidelity.json");
writeFileSync(mPath, `${JSON.stringify(measurementsDoc, null, 2)}\n`);
writeFileSync(rPath, `${JSON.stringify(resultsDoc, null, 2)}\n`);

const inconclusiveCount = measurements.filter(
  (m) => m.status === "inconclusive",
).length;
console.log(
  `[fidelity-measurements] ${fixtures.length} fixture(s) read from ${inDir}\n` +
    `  evidence rows : ${results.length} -> ${rPath}\n` +
    `  measurements  : ${measurements.length}` +
    (inconclusiveCount
      ? ` (${inconclusiveCount} INCONCLUSIVE — excluded from any trend)`
      : "") +
    ` -> ${mPath}` +
    (unreadable.length
      ? `\n  UNREADABLE    : ${unreadable.join(", ")}`
      : ""),
);
