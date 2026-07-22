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
import { Progress, ProgressLabel, ProgressValue } from "@/components/ui/progress";
import type { ToolsState, VendorActivity } from "@/lib/bindings";
import { formatFileSize } from "@/lib/format/format";
import {
  checkForUpdate,
  openReleasePage,
  scrubLogs,
  vendorCheck,
  vendorInstall,
} from "@/lib/ipc/settings";
import { useAppStore } from "@/lib/store/app-store";

import { SettingContainer, SettingsGroup } from "./settings-primitives";

function message(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

function isBusy(activity: VendorActivity): boolean {
  return (
    activity === "Checking" ||
    activity === "Installing" ||
    (typeof activity === "object" && "Downloading" in activity)
  );
}

function activityLabel(activity: VendorActivity): string {
  if (activity === "Idle") return "Idle";
  if (activity === "Checking") return "Checking for dependencies…";
  if (activity === "Installing") return "Installing dependencies…";
  if ("Downloading" in activity) return "Downloading dependencies…";
  return `Dependency operation failed: ${activity.Failed.detail}`;
}

function availabilityLabel(tools: ToolsState | null): string {
  if (tools === null) return "Waiting for dependency status";
  if ("Missing" in tools.availability) {
    const names = tools.availability.Missing.missing.join(", ");
    return `${names || "Media dependencies"} missing. ${tools.availability.Missing.detail}`;
  }
  const available = tools.availability.Available;
  const revisions = available.revisions;
  return `${available.source} tools — FFmpeg ${revisions.ffmpeg}, encoder ${revisions.encoder}`;
}

export function DependenciesActions() {
  const tools = useAppStore((state) => state.tools);
  const session = useAppStore((state) => state.session);
  const hasActiveItem = useAppStore((state) =>
    state.durable.queue.some(
      (item) =>
        typeof item.state === "object" &&
        ("Reserved" in item.state || "Claimed" in item.state || "Running" in item.state),
    ),
  );
  const [commandPending, setCommandPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const activity = tools?.activity ?? "Idle";
  const busy = tools === null || commandPending || isBusy(activity);
  const missing = tools !== null && "Missing" in tools.availability;
  const failed = typeof activity === "object" && "Failed" in activity;
  const showInstall = missing || tools?.update_available === true || failed;
  const installDisabled = busy || session !== "Idle" || hasActiveItem;
  const installLabel = missing ? "Install" : tools?.update_available ? "Update" : "Retry install";

  const run = async (operation: () => Promise<void>) => {
    setCommandPending(true);
    setError(null);
    try {
      await operation();
    } catch (commandError: unknown) {
      setError(message(commandError, "Dependency action failed"));
    } finally {
      setCommandPending(false);
    }
  };

  const downloading = typeof activity === "object" && "Downloading" in activity;
  const download = downloading ? activity.Downloading : null;
  const percent =
    download?.total !== null && download?.total !== undefined && download.total > 0
      ? Math.min(100, (download.received / download.total) * 100)
      : null;

  return (
    <SettingsGroup title="Dependencies">
      <SettingContainer label="Media tools" description={availabilityLabel(tools)}>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" disabled={busy} onClick={() => void run(vendorCheck)}>
            {activity === "Checking" ? "Checking…" : "Check"}
          </Button>
          {showInstall && (
            <Button size="sm" disabled={installDisabled} onClick={() => void run(vendorInstall)}>
              {installLabel}
            </Button>
          )}
        </div>
      </SettingContainer>
      <SettingContainer label="Dependency activity" description={activityLabel(activity)} last>
        {download !== null ? (
          <Progress
            className="w-56"
            value={percent}
            role="progressbar"
            aria-valuenow={percent ?? undefined}
          >
            <ProgressLabel>Download</ProgressLabel>
            <ProgressValue>
              {download.total === null
                ? formatFileSize(download.received)
                : `${formatFileSize(download.received)} / ${formatFileSize(download.total)}`}
            </ProgressValue>
          </Progress>
        ) : (
          <p className="max-w-72 text-right text-xs text-muted-foreground" role="status">
            {error ?? activityLabel(activity)}
          </p>
        )}
      </SettingContainer>
    </SettingsGroup>
  );
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
      <SettingContainer label="Application updates" description={status} last>
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
