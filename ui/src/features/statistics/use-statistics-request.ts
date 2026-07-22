import { useCallback, useEffect, useRef, useState } from "react";

import { useViewActive } from "@/components/layout/view-activity";
import type { StatisticsPayload } from "@/lib/bindings";
import { requestStatistics } from "@/lib/ipc";
import { useAppStore } from "@/lib/store/app-store";

export type StatisticsRequestPhase = "idle" | "loading" | "refreshing";

export interface StatisticsRequestState {
  payload: StatisticsPayload | null;
  phase: StatisticsRequestPhase;
  error: string | null;
  utcOffsetMinutes: number;
}

interface PendingRequest {
  id: number;
  offset: number;
  source: StatisticsPayload | null;
}

/** JavaScript offsets use the opposite sign from the engine contract. */
export function currentUtcOffsetMinutes(): number {
  return -new Date().getTimezoneOffset();
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "Statistics could not be requested";
}

/**
 * Request-driven Statistics lifecycle. Command acknowledgement only confirms
 * acceptance; a new matching payload reference on the stream completes the
 * request. Existing matching data remains visible while it refreshes.
 */
export function useStatisticsRequest(): StatisticsRequestState {
  const active = useViewActive();
  const statistics = useAppStore((state) => state.statistics);
  const snapshotGeneration = useAppStore((state) => state.snapshotGeneration);
  const statisticsRef = useRef(statistics);
  statisticsRef.current = statistics;

  const pendingRef = useRef<PendingRequest | null>(null);
  const nextRequestId = useRef(0);
  const seenSnapshotGeneration = useRef(snapshotGeneration);
  const [phase, setPhase] = useState<StatisticsRequestPhase>("idle");
  const [error, setError] = useState<string | null>(null);

  const requestFresh = useCallback(() => {
    const offset = currentUtcOffsetMinutes();
    const current = statisticsRef.current;
    const pending = pendingRef.current;
    if (pending !== null && pending.offset === offset) {
      return;
    }

    const id = nextRequestId.current + 1;
    nextRequestId.current = id;
    pendingRef.current = { id, offset, source: current };
    setPhase(current?.utc_offset_minutes === offset ? "refreshing" : "loading");
    setError(null);

    void requestStatistics(offset).catch((requestError: unknown) => {
      if (pendingRef.current?.id !== id) {
        return;
      }
      pendingRef.current = null;
      setPhase("idle");
      setError(errorMessage(requestError));
    });
  }, []);

  // Every replay must supersede a request from the replaced connection, even
  // when Statistics was already null and that selector therefore did not
  // change. This effect intentionally precedes the activation effect so the
  // latter deduplicates against the replay-triggered request.
  useEffect(() => {
    const changed = seenSnapshotGeneration.current !== snapshotGeneration;
    seenSnapshotGeneration.current = snapshotGeneration;
    if (!active || !changed) {
      return;
    }
    pendingRef.current = null;
    requestFresh();
  }, [active, requestFresh, snapshotGeneration]);

  // Activity cleans this effect while hidden and restarts it on activation.
  useEffect(() => {
    if (active) {
      requestFresh();
    }
  }, [active, requestFresh]);

  useEffect(() => {
    if (!active) {
      return;
    }

    const offset = currentUtcOffsetMinutes();
    const pending = pendingRef.current;
    if (statistics?.utc_offset_minutes === offset) {
      if (pending !== null && statistics !== pending.source) {
        pendingRef.current = null;
        setPhase("idle");
        setError(null);
      }
      return;
    }

    // A snapshot clears the non-replayed answer. If it interrupted a refresh
    // that started from valid data, the old request belongs to the replaced
    // connection and must not suppress the reconnect request.
    if (statistics === null && pending !== null && pending.source !== null) {
      pendingRef.current = null;
    }
    requestFresh();
  }, [active, requestFresh, statistics]);

  useEffect(() => {
    if (!active) {
      return;
    }
    const onFocus = () => requestFresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [active, requestFresh]);

  const utcOffsetMinutes = currentUtcOffsetMinutes();
  const payload = statistics?.utc_offset_minutes === utcOffsetMinutes ? statistics : null;

  return { payload, phase, error, utcOffsetMinutes };
}
