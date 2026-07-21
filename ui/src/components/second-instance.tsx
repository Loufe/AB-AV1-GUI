/**
 * Full-window replacement shown when another CRFty instance holds the data
 * lock (ADR-008). This process must never touch the shared queue, history,
 * or settings, so no view is mounted at all — the only remedy is to close
 * this window and use the running one.
 */
export function SecondInstanceScreen({ lockPath }: { lockPath: string }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 p-8">
      <p className="text-lg">CRFty is already running</p>
      <p className="max-w-xl text-center text-sm text-muted-foreground">
        Another CRFty window owns this computer&apos;s queue and history. Close this window and keep
        using the one that is already open.
      </p>
      <p className="selectable max-w-xl text-center text-xs text-muted-foreground">
        Data lock held at {lockPath}
      </p>
    </div>
  );
}
