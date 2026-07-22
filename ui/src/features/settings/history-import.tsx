import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { pickPath } from "@/lib/ipc/path-picker";
import { importHistory } from "@/lib/ipc/settings";

import { SettingContainer, SettingsGroup } from "./settings-primitives";

/**
 * One-shot history adoption: the user points at a file produced by the V2
 * converter script and the engine parks its records durably. Independent of
 * the draft/save settings flow — the import is its own engine command.
 */
export function HistoryImport() {
  const [path, setPath] = useState("");
  const [picking, setPicking] = useState(false);
  const [importing, setImporting] = useState(false);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const runImport = async () => {
    setImporting(true);
    setOutcome(null);
    setError(null);
    try {
      const summary = await importHistory(path.trim());
      setOutcome(`Parked ${summary.parked}, skipped ${summary.skipped}`);
    } catch (importError: unknown) {
      setError(importError instanceof Error ? importError.message : "History import failed");
    } finally {
      setImporting(false);
    }
  };

  const browse = async () => {
    setPicking(true);
    setError(null);
    try {
      const selected = await pickPath("HistoryImport");
      if (selected !== null) setPath(selected);
    } catch (pickerError: unknown) {
      setError(pickerError instanceof Error ? pickerError.message : "History picker failed");
    } finally {
      setPicking(false);
    }
  };

  return (
    <SettingsGroup title="History">
      <SettingContainer
        label="Import history"
        description="Import a file produced by the bundled converter script (tools/export_history_v3.py)"
        htmlFor="history-import-path"
        last
      >
        <div className="flex flex-col items-end gap-1">
          <div className="flex gap-2">
            <Input
              id="history-import-path"
              className="w-72"
              value={path}
              placeholder="Path to the exported history file"
              disabled={importing || picking}
              onChange={(event) => setPath(event.target.value)}
            />
            <Button
              variant="outline"
              disabled={importing || picking}
              aria-label="Choose history export"
              onClick={() => void browse()}
            >
              {picking ? "Choosing…" : "Browse…"}
            </Button>
            <Button
              variant="outline"
              disabled={importing || picking || path.trim().length === 0}
              onClick={runImport}
            >
              {importing ? "Importing…" : "Import"}
            </Button>
          </div>
          {outcome !== null && (
            <p className="text-xs text-muted-foreground" role="status">
              {outcome}
            </p>
          )}
          {error !== null && (
            <p className="text-xs text-destructive" role="alert">
              {error}
            </p>
          )}
        </div>
      </SettingContainer>
    </SettingsGroup>
  );
}
