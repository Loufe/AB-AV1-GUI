import { useState } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { checkForUpdate, openReleasePage, scrubLogs } from "@/lib/ipc/settings";

import { SettingContainer, SettingsGroup } from "./settings-primitives";

function message(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

export function ApplicationUpdateAction() {
  const [checking, setChecking] = useState(false);
  const [summary, setSummary] = useState<Awaited<ReturnType<typeof checkForUpdate>> | null>(null);
  const [error, setError] = useState<string | null>(null);

  const check = async () => {
    setChecking(true);
    setError(null);
    try {
      setSummary(await checkForUpdate());
    } catch (checkError: unknown) {
      setError(message(checkError, "Update check failed"));
    } finally {
      setChecking(false);
    }
  };

  const open = async () => {
    setError(null);
    try {
      await openReleasePage();
    } catch (openError: unknown) {
      setError(message(openError, "Release page could not be opened"));
    }
  };

  const status =
    error ??
    (summary === null
      ? "Updates are checked only when requested."
      : summary.update_available
        ? `Version ${summary.latest} is available; this app is ${summary.current}.`
        : `Version ${summary.current} is current.`);

  return (
    <SettingsGroup title="Application">
      <SettingContainer
        label="Application updates"
        description="Check releases manually; updates are never installed automatically"
        last
      >
        <div className="flex flex-col items-end gap-1">
          <div className="flex gap-2">
            <Button variant="outline" size="sm" disabled={checking} onClick={() => void check()}>
              {checking ? "Checking…" : "Check for updates"}
            </Button>
            {summary?.update_available === true && (
              <Button size="sm" onClick={() => void open()}>
                Open release page
              </Button>
            )}
          </div>
          <p
            className={
              error === null ? "text-xs text-muted-foreground" : "text-xs text-destructive"
            }
            role={error === null ? "status" : "alert"}
          >
            {status}
          </p>
        </div>
      </SettingContainer>
    </SettingsGroup>
  );
}

export function PrivacyMaintenanceActions() {
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [scrubbing, setScrubbing] = useState(false);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const runScrub = async () => {
    setConfirmOpen(false);
    setScrubbing(true);
    setOutcome(null);
    setError(null);
    try {
      const summary = await scrubLogs();
      setOutcome(
        `Examined ${summary.total} log files; rewrote ${summary.modified}; ${summary.failed} failed.`,
      );
    } catch (scrubError: unknown) {
      setError(message(scrubError, "Log scrub failed"));
    } finally {
      setScrubbing(false);
    }
  };

  return (
    <SettingsGroup title="Privacy maintenance">
      <SettingContainer
        label="Scrub existing logs"
        description="Irreversibly replace recognizable paths in logs already on disk"
      >
        <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
          <AlertDialogTrigger render={<Button variant="outline" size="sm" disabled={scrubbing} />}>
            {scrubbing ? "Scrubbing…" : "Scrub logs"}
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Scrub existing logs?</AlertDialogTitle>
              <AlertDialogDescription>
                This permanently rewrites log files and cannot be undone. Conversion paths are
                replaced with anonymized forms even when live log anonymization is off.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={() => void runScrub()}>Scrub logs</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </SettingContainer>
      <SettingContainer
        label="Scrub existing history"
        description="History privacy support is not available in this build yet"
        last
      >
        <Button variant="outline" size="sm" disabled>
          Unavailable
        </Button>
      </SettingContainer>
      {(outcome !== null || error !== null) && (
        <div className="border-t border-border px-4 py-2 text-xs" role="status">
          <span className={error === null ? "text-muted-foreground" : "text-destructive"}>
            {error ?? outcome}
          </span>
        </div>
      )}
    </SettingsGroup>
  );
}
