import { describe, expect, it } from "vitest";

import type {
  DurableDelta,
  DurableState_Deserialize,
  EphemeralDelta,
  FileRecord_Deserialize,
  Settings,
  Telemetry,
} from "@/lib/bindings";

import { foldConfig, foldDurable, foldSession, foldTelemetry } from "./fold";
import fixturesJson from "./fold-fixtures.json";

interface Scenario {
  name: string;
  initial: DurableState_Deserialize;
  deltas: DurableDelta[];
  expected: DurableState_Deserialize;
}

// resolveJsonModule infers wide literal types (e.g. string where the binding
// union expects "Analyze" | "Convert"), so the generated file is cast to the
// binding types it was serialized from.
const fixtures = fixturesJson as unknown as { scenarios: Scenario[] };

// Rust stores per-record analyses in a BTreeMap sorted by AnalysisProfile's
// Ord; the TS fold appends newly seen profiles instead. Entry order is not
// semantic (lookups scan by profile equality), so records compare with
// analyses sorted by their serialized profile.
function normalized(state: DurableState_Deserialize): DurableState_Deserialize {
  const records = Object.fromEntries(
    Object.entries(state.records).map(([key, record]: [string, FileRecord_Deserialize]) => [
      key,
      {
        ...record,
        analyses: [...record.analyses].sort((a, b) =>
          JSON.stringify(a[0]).localeCompare(JSON.stringify(b[0])),
        ),
      },
    ]),
  );
  return { ...state, records };
}

function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === "object") {
    for (const entry of Object.values(value)) {
      deepFreeze(entry);
    }
    Object.freeze(value);
  }
  return value;
}

function replay(scenario: Scenario): DurableState_Deserialize {
  let state = scenario.initial;
  for (const delta of scenario.deltas) {
    state = foldDurable(state, delta);
  }
  return state;
}

describe("foldDurable (Rust fold parity)", () => {
  it("covers every fixture scenario", () => {
    expect(fixtures.scenarios.length).toBeGreaterThan(0);
  });

  for (const scenario of fixtures.scenarios) {
    it(scenario.name, () => {
      expect(normalized(replay(scenario))).toEqual(normalized(scenario.expected));
    });
  }
});

describe("foldDurable immutability", () => {
  for (const scenario of fixtures.scenarios) {
    it(`${scenario.name} leaves its input untouched`, () => {
      // Frozen state throws on any in-place mutation; replaying against the
      // frozen copy proves every arm builds new structures instead.
      const pristine = structuredClone(scenario.initial);
      const frozen = deepFreeze(structuredClone(scenario.initial));
      replay({ ...scenario, initial: frozen });
      expect(frozen).toEqual(pristine);
    });
  }
});

function settings(hardwareDecode: boolean): Settings {
  return {
    last_input_folder: null,
    scan_extensions: ["mp4", "mkv", "avi", "wmv"],
    output: {
      default_mode: "replace",
      suffix: "_av1",
      separate_folder: null,
      overwrite_existing: false,
    },
    hardware_decode: hardwareDecode,
    privacy: {
      anonymize_logs: false,
      anonymize_history: false,
    },
    log_folder: null,
  };
}

function telemetry(runId: number, sequence: number): Telemetry {
  return {
    run_id: runId,
    sequence,
    phase: "Encoding",
    progress: { OutputPositionMs: 5_000 },
    fps_centi: null,
    eta_ms: null,
  };
}

describe("foldConfig", () => {
  it("replaces the settings wholesale", () => {
    const changed = settings(false);
    expect(foldConfig(settings(true), { SettingsChanged: { settings: changed } })).toBe(changed);
    expect(foldConfig(null, { SettingsChanged: { settings: changed } })).toBe(changed);
  });
});

describe("foldSession", () => {
  it("applies SessionChanged and passes other ephemerals through", () => {
    const changed: EphemeralDelta = { SessionChanged: "Running" };
    const unrelated: EphemeralDelta = { Telemetry: telemetry(1, 1) };
    expect(foldSession("Idle", changed)).toBe("Running");
    expect(foldSession("Idle", unrelated)).toBe("Idle");
  });
});

describe("foldTelemetry", () => {
  it("keeps the latest telemetry per run and removes cleared runs", () => {
    const first = telemetry(1, 1);
    const second = telemetry(1, 2);
    const other = telemetry(2, 1);

    let state = foldTelemetry({}, { Telemetry: first });
    state = foldTelemetry(state, { Telemetry: other });
    expect(state).toEqual({ 1: first, 2: other });

    state = foldTelemetry(state, { Telemetry: second });
    expect(state).toEqual({ 1: second, 2: other });

    state = foldTelemetry(state, { TelemetryCleared: { run_id: 1 } });
    expect(state).toEqual({ 2: other });
  });

  it("passes non-telemetry ephemerals through by reference", () => {
    const state = { 1: telemetry(1, 1) };
    expect(foldTelemetry(state, { SessionChanged: "Idle" })).toBe(state);
    expect(foldTelemetry(state, { WorkerCrashed: { message: "boom" } })).toBe(state);
  });
});
