import { describe, expect, it } from "vitest";

import type { Settings, StreamPayload } from "@/lib/bindings";

import { foldSettings } from "./settings-stream";

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

describe("foldSettings", () => {
  it("takes settings from the ordered snapshot and config events only", () => {
    const initial = settings(true);
    const changed = settings(false);
    const snapshot = {
      Snapshot: {
        durable: {
          queue: [],
          paths: {},
          records: {},
          outputs: {},
          conversion_runs: {},
        },
        settings: initial,
      },
    } satisfies StreamPayload;
    const config = {
      Config: { SettingsChanged: { settings: changed } },
    } satisfies StreamPayload;
    const unrelated = {
      Degraded: { reason: "fixture" },
    } satisfies StreamPayload;

    expect(foldSettings(null, snapshot)).toEqual(initial);
    expect(foldSettings(initial, config)).toEqual(changed);
    expect(foldSettings(changed, unrelated)).toBe(changed);
  });
});
