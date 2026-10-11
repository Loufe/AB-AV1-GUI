import type {
  AnalysisActivity,
  AnalysisGeneration_Deserialize,
  AnalysisRow,
  AnalysisRowStatus,
  Settings,
  ToolAvailability,
} from "@/lib/bindings";

export function analysisSettings(): Settings {
  return {
    last_input_folder: "/videos",
    scan_extensions: ["mkv", "mp4"],
    output: {
      default_mode: "suffix",
      suffix: "_av1",
      separate_folder: null,
      overwrite_existing: false,
    },
    hardware_decode: true,
    privacy: { anonymize_logs: false, anonymize_history: false },
    log_folder: null,
    tools: { ffmpeg: null, ffprobe: null },
  };
}

export function analysisTools(): ToolAvailability {
  return {
    Located: {
      tools: {
        ffmpeg: { path: "/fixture/ffmpeg", source: "SearchPath" },
        ffprobe: { path: "/fixture/ffprobe", source: "SearchPath" },
      },
      verification: {
        Verified: {
          revisions: { ab_av1: "fixture", ffmpeg: "fixture", encoder: "fixture" },
          hardware_decoders: [],
        },
      },
    },
  };
}

export function discoveredRow(id = 2, name = "movie.mkv"): AnalysisRow {
  return {
    id,
    parent: 1,
    display_name: { text: name, lossy: false },
    display_path: { text: `/videos/series/${name}`, lossy: false },
    entry: { File: { scan: "Discovered" } },
  };
}

export function scannedRow(id = 2, name = "movie.mkv", status?: AnalysisRowStatus): AnalysisRow {
  return {
    ...discoveredRow(id, name),
    entry: {
      File: {
        scan: {
          Scanned: {
            content_key: `content-${id}`,
            refresh_failure: null,
            metadata: {
              codec: "H264",
              container: "Matroska",
              width: 1920,
              height: 1080,
              rotation_degrees: 0,
              duration_ms: 120_000,
              size_bytes: 20_000_000,
              audio: [],
              subtitle_count: 0,
            },
            status: status ?? {
              applicable: { Scanned: { reuse: "Unanalyzed" } },
              historical: "Scanned",
              analyze: "Eligible",
              convert: "Eligible",
            },
          },
        },
      },
    },
  };
}

export function predictedRow(id = 2, name = "movie.mkv"): AnalysisRow {
  return scannedRow(id, name, {
    applicable: {
      Analyzed: {
        prediction: {
          metric: "Vmaf",
          crf: 28_000,
          score: 9650,
          predicted_size: 8_000_000,
          predicted_percent_basis_points: 4000,
          predicted_duration_ms: 90_000,
          requested_target: 95,
          successful_target: 95,
        },
      },
    },
    historical: "Analyzed",
    analyze: "Eligible",
    convert: "Eligible",
  });
}

export function convertedRow(): AnalysisRow {
  const row = scannedRow();
  if (row.entry.File?.scan === "Discovered" || row.entry.File?.scan.Scanned === undefined)
    throw new Error("Scanned fixture missing metadata");
  return {
    ...row,
    entry: {
      File: {
        scan: {
          SettledOutput: {
            source_content_key: "source",
            output_content_key: "output",
            metadata: {
              ...row.entry.File.scan.Scanned.metadata,
              codec: "Av1",
              size_bytes: 8_000_000,
            },
            refresh_failure: null,
            status: {
              applicable: {
                Converted: {
                  summary: {
                    Encoded: {
                      input_size: 20_000_000,
                      output_size: 8_000_000,
                      encoding_time: 90_000,
                      crf: 28_000,
                      metric: "Vmaf",
                      score: 9650,
                      target: 95,
                      decided_at: 1,
                      source_run: 1,
                    },
                  },
                },
              },
              historical: "Converted",
              analyze: { Skip: { reason: { AlreadyConverted: { source_run: 1 } } } },
              convert: { Skip: { reason: { AlreadyConverted: { source_run: 1 } } } },
            },
          },
        },
      },
    },
  };
}

export function analysisGeneration(
  rows: AnalysisRow[] = [discoveredRow()],
  activity: AnalysisActivity = "Discovered",
  id = 7,
): AnalysisGeneration_Deserialize {
  return { id, rows, activity, roots: [{ text: "/videos", lossy: false }] };
}
