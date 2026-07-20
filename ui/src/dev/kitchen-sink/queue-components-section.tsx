import { useState } from "react";

import type { QueueItem, QueueItemId } from "@/lib/bindings";
import {
  NowProcessingCard,
  QueueTable,
  QueueToolbar,
  SelectionCard,
  deriveRowStatus,
  moveRowBefore,
} from "@/features/queue";
import type { QueueRowData } from "@/features/queue";
import { formatCompactTime, formatCrf, formatFileSize, formatTime } from "@/lib/format/format";

import { Section, ThemePair } from "./theme-pair";

/**
 * The REAL queue components (features/queue) fed with mock rows — the wired
 * successor to the D11 static pass above it. Data flows through the actual
 * bindings types and deriveRowStatus, so this page is the visual test bed
 * until the store wiring lands.
 */

const GIB = 1024 ** 3;

function item(
  id: number,
  name: string,
  operation: QueueItem["operation"],
  state: QueueItem["state"],
): QueueItem {
  return {
    id: id as QueueItemId,
    input: `C:\\Videos\\Season 1\\${name}`,
    operation,
    output_target: operation === "Convert" ? "Replace" : { Suffix: { suffix: "_av1" } },
    state,
  };
}

const ENCODE_DURATION_MS = 120_000;

const ROWS: QueueRowData[] = [
  {
    item: item(1, "s01e01.mkv", "Convert", { Finished: "Converted" }),
    streams: "H264 / AAC",
    sizeBytes: 3.21 * GIB,
    timeSec: 4320,
    timeConfidence: "exact",
    preciseCrf: false,
    status: deriveRowStatus({ Finished: "Converted" }, null, null, 1.87 * GIB),
  },
  {
    item: item(2, "s01e02.mkv", "Convert", { Running: { claim_id: 2, run_id: 2 } }),
    streams: "H264 / AAC, AC3",
    sizeBytes: 4.11 * GIB,
    timeSec: 6780,
    timeConfidence: "exact",
    preciseCrf: true,
    status: deriveRowStatus(
      { Running: { claim_id: 2, run_id: 2 } },
      { run_id: 2, sequence: 41, phase: "Encoding", progress: { OutputPositionMs: 74_400 } },
      ENCODE_DURATION_MS,
      null,
    ),
  },
  {
    item: item(3, "s01e03.mkv", "Convert", "Queued"),
    streams: "H264 / AAC",
    sizeBytes: 2.87 * GIB,
    timeSec: 3480,
    timeConfidence: "estimate",
    preciseCrf: true,
    status: deriveRowStatus("Queued", null, null, null),
  },
  {
    item: item(4, "s01e04.mkv", "Analyze", "Queued"),
    streams: "HEVC / AC3",
    sizeBytes: 5.63 * GIB,
    timeSec: 5040,
    timeConfidence: "rough",
    preciseCrf: false,
    status: deriveRowStatus("Queued", null, null, null),
  },
  {
    item: item(5, "s01e05.avi", "Convert", {
      Finished: {
        NotWorthwhile: {
          attempts: [
            {
              target: 90,
              last_measurement: {
                crf: 30,
                score: 89.6,
                predicted_size: 0.7 * GIB,
                predicted_percent_basis_points: 9700,
                predicted_duration_ms: 900_000,
                from_cache: false,
              },
            },
          ],
        },
      },
    }),
    streams: "MPEG4 / MP3",
    sizeBytes: 0.72 * GIB,
    timeSec: null,
    timeConfidence: "exact",
    preciseCrf: false,
    status: deriveRowStatus(
      {
        Finished: {
          NotWorthwhile: {
            attempts: [
              {
                target: 90,
                last_measurement: {
                  crf: 30,
                  score: 89.6,
                  predicted_size: 0.7 * GIB,
                  predicted_percent_basis_points: 9700,
                  predicted_duration_ms: 900_000,
                  from_cache: false,
                },
              },
            ],
          },
        },
      },
      null,
      null,
      null,
    ),
  },
  {
    item: item(6, "s01e06.wmv", "Convert", {
      Finished: {
        Failed: { kind: "EncodeStart", message: "ffprobe: moov atom not found", diagnostic: "" },
      },
    }),
    streams: "VC1 / WMAV2",
    sizeBytes: 1.38 * GIB,
    timeSec: null,
    timeConfidence: "exact",
    preciseCrf: false,
    status: deriveRowStatus(
      {
        Finished: {
          Failed: { kind: "EncodeStart", message: "ffprobe: moov atom not found", diagnostic: "" },
        },
      },
      null,
      null,
      null,
    ),
  },
];

function QueuePanel() {
  const [rows, setRows] = useState(ROWS);
  const [selectedId, setSelectedId] = useState<QueueItemId | null>(4 as QueueItemId);
  const selected = rows.find((row) => row.item.id === selectedId) ?? null;
  return (
    <div className="flex flex-col gap-3">
      <QueueToolbar session="Running" queueEmpty={false} hasSelection={selected !== null} />
      <QueueTable
        rows={rows}
        selectedId={selectedId}
        onSelect={setSelectedId}
        onMove={(itemId, beforeId) =>
          setRows((current) => moveRowBefore(current, itemId, beforeId))
        }
      />
      <div className="grid grid-cols-[2fr_1fr] gap-3">
        <NowProcessingCard
          name="s01e02.mkv"
          analyzePercent={100}
          encodePercent={62}
          detail={`VMAF 95.3 · CRF ${formatCrf(24.25)} · preset 6 · output ${formatFileSize(1.21 * GIB)} so far · elapsed ${formatTime(2533)} · ETA ${formatCompactTime(2280)}`}
        />
        {selected !== null && <SelectionCard row={selected} />}
      </div>
    </div>
  );
}

export function QueueComponentsSection() {
  return (
    <Section title="Queue components (real, mock-fed)">
      <p className="text-sm text-muted-foreground">
        These are the production components from features/queue driven by bindings-typed mock data
        through deriveRowStatus. Row click selects (drives the properties card); toolbar reflects a
        Running session. Wiring to the event stream is a later workstream.
      </p>
      <ThemePair>
        <QueuePanel />
      </ThemePair>
    </Section>
  );
}
