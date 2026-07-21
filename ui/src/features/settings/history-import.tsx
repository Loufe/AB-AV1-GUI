import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { importHistory } from "@/lib/ipc";

import { SettingContainer, SettingsGroup } from "./settings-primitives";

/**
 * One-shot history adoption: the user points at a file produced by the V2
 * converter script and the engine parks its records durably. Independent of
 * the draft/save settings flow — the import is its own engine command.
 */
export function HistoryImport() {
  const [path, setPath] = useState("");
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

  return (
    <SettingsGroup title="History">
      <SettingContainer
        label="Import history"
        description="Import a file produced by the bundled converter script (tools/export_history_v3.py)"
        last
      >
        <div className="flex flex-col items-end gap-1">
          <div className="flex gap-2">
            <Input
              className="w-72"
              value={path}
              placeholder="Path to the exported history file"
              disabled={importing}
              onChange={(event) => setPath(event.target.value)}
            />
            <Button
              variant="outline"
              disabled={importing || path.trim().length === 0}
              onClick={runImport}
            >
              {importing ? "Importing…" : "Import"}
            </Button>
          </div>
          {outcome !== null && <p className="text-xs text-muted-foreground">{outcome}</p>}
          {error !== null && <p className="text-xs text-destructive">{error}</p>}
        </div>
      </SettingContainer>
    </SettingsGroup>
  );
}
