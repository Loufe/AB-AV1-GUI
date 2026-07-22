import {
  commands,
  type ImportSummary,
  type ReleaseSummary,
  type ScrubSummary,
  type Settings,
} from "@/lib/bindings";

export async function saveSettings(settings: Settings): Promise<void> {
  const result = await commands.setSettings(settings);
  if (result.status === "error") {
    throw new Error(`settings save failed (${result.error.code}): ${result.error.message}`);
  }
}

/** Import a V2 history export; accepted records enter the durable parked inbox. */
export async function importHistory(path: string): Promise<ImportSummary> {
  const result = await commands.importHistory(path);
  if (result.status === "error") {
    throw new Error(`history import failed (${result.error.code}): ${result.error.message}`);
  }
  return result.data;
}

export async function vendorCheck(): Promise<void> {
  const result = await commands.vendorCheck();
  if (result.status === "error") {
    throw new Error(`dependency check failed (${result.error.code}): ${result.error.message}`);
  }
}

export async function vendorInstall(): Promise<void> {
  const result = await commands.vendorInstall();
  if (result.status === "error") {
    throw new Error(`dependency install failed (${result.error.code}): ${result.error.message}`);
  }
}

export async function scrubLogs(): Promise<ScrubSummary> {
  const result = await commands.scrubLogs();
  if (result.status === "error") {
    throw new Error(`log scrub failed (${result.error.code}): ${result.error.message}`);
  }
  return result.data;
}

export async function checkForUpdate(): Promise<ReleaseSummary> {
  const result = await commands.checkForUpdate();
  if (result.status === "error") {
    throw new Error(`update check failed (${result.error.code}): ${result.error.message}`);
  }
  return result.data;
}

export async function openReleasePage(): Promise<void> {
  const result = await commands.openReleasePage();
  if (result.status === "error") {
    throw new Error(`open release page failed (${result.error.code}): ${result.error.message}`);
  }
}
