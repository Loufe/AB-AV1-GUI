import { describe, expect, it } from "vitest";

import type { DurableState_Deserialize, HistoryRow } from "@/lib/bindings";

import { historyRows } from "./history-rows";
import fixturesJson from "./projection-fixtures.json";

interface Scenario {
  name: string;
  state: DurableState_Deserialize;
  expected_rows: HistoryRow[];
}

// resolveJsonModule infers wide literal types (e.g. string where the binding
// union expects "Convert"), so the generated file is cast to the binding
// types it was serialized from.
const fixtures = fixturesJson as unknown as { scenarios: Scenario[] };

function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === "object") {
    for (const entry of Object.values(value)) {
      deepFreeze(entry);
    }
    Object.freeze(value);
  }
  return value;
}

describe("historyRows (Rust projection parity)", () => {
  it("covers every fixture scenario", () => {
    expect(fixtures.scenarios.length).toBeGreaterThan(0);
  });

  for (const scenario of fixtures.scenarios) {
    it(scenario.name, () => {
      expect(historyRows(scenario.state)).toEqual(scenario.expected_rows);
    });
  }
});

describe("historyRows purity", () => {
  for (const scenario of fixtures.scenarios) {
    it(`${scenario.name} leaves its input untouched`, () => {
      // Frozen state throws on any in-place mutation; deriving from the
      // frozen copy proves the projection builds new structures only.
      const pristine = structuredClone(scenario.state);
      const frozen = deepFreeze(structuredClone(scenario.state));
      historyRows(frozen);
      expect(frozen).toEqual(pristine);
    });
  }
});
