import type { RunTotals, StatisticsPayload } from "@/lib/bindings";

type StatisticsOverrides = Partial<Omit<StatisticsPayload, "runs">> & {
  runs?: Partial<RunTotals>;
};

const EMPTY_RUNS: RunTotals = {
  analyzed: 0,
  converted: 0,
  remuxed: 0,
  not_worthwhile: 0,
  stopped: 0,
  skipped: 0,
  failed: 0,
};

export function statisticsPayload(overrides: StatisticsOverrides = {}): StatisticsPayload {
  return {
    utc_offset_minutes: 0,
    converted_files: 0,
    sized_converted_files: 0,
    remuxed_files: 0,
    not_worthwhile_files: 0,
    total_input_bytes: 0,
    total_output_bytes: 0,
    total_saved_bytes: 0,
    remux_saved_bytes: 0,
    total_time_ms: 0,
    gigabytes_per_hour: null,
    reduction_percent: null,
    vmaf: null,
    crf: null,
    reduction_bins: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    grew_count: 0,
    codecs: [],
    cumulative_savings: [],
    first_epoch_day: null,
    last_epoch_day: null,
    ...overrides,
    runs: { ...EMPTY_RUNS, ...overrides.runs },
  };
}
