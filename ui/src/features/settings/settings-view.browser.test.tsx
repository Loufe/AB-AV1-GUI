import { page } from "vitest/browser";
import { describe, expect, it } from "vitest";

import type { Settings, ToolsState } from "@/lib/bindings";
import { appStore } from "@/lib/store/app-store";
import { renderApp } from "@/test/browser/render";
import { installTauriMock } from "@/test/browser/tauri";

import { SettingsView } from "./settings-view";

function settings(overrides: Partial<Settings> = {}): Settings {
  return {
    last_input_folder: null,
    scan_extensions: ["mp4", "mkv", "avi", "wmv"],
    output: {
      default_mode: "replace",
      suffix: "_av1",
      separate_folder: null,
      overwrite_existing: false,
    },
    hardware_decode: true,
    privacy: { anonymize_logs: false, anonymize_history: false },
    log_folder: null,
    ...overrides,
  };
}

function missingTools(): ToolsState {
  return {
    availability: {
      Missing: { missing: ["Ffmpeg", "Ffprobe"], detail: "managed tools are not installed" },
    },
    activity: "Idle",
    update_available: false,
  };
}

function availableTools(updateAvailable = false): ToolsState {
  return {
    availability: {
      Available: {
        source: "Managed",
        revisions: { ab_av1: "0.11.1", ffmpeg: "8.0", encoder: "3.2" },
      },
    },
    activity: "Idle",
    update_available: updateAvailable,
  };
}

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

describe("Settings view", () => {
  it("discards locally, then waits for the authoritative settings delta after save ack", async () => {
    const committed = settings();
    const tauri = installTauriMock({ set_settings: () => null });
    await renderApp(<SettingsView />, { appState: { settings: committed } });

    const inputFolder = page.getByRole("textbox", { name: "Input folder", exact: true });
    await inputFolder.fill("/draft/one");
    await page.getByRole("button", { name: "Discard changes" }).click();
    await expect.element(inputFolder).toHaveValue("");

    await inputFolder.fill("/draft/two");
    await page.getByRole("button", { name: "Save changes" }).click();
    await expect.element(page.getByRole("button", { name: "Saving…" })).toBeDisabled();

    const submitted = { ...committed, last_input_folder: "/draft/two" };
    expect(tauri.callsFor("set_settings").at(0)?.payload).toEqual({ settings: submitted });
    expect(appStore.getState().settings).toEqual(committed);

    appStore.setState({ settings: submitted });
    await expect.element(page.getByRole("button", { name: "Save changes" })).toBeDisabled();
    await expect.element(inputFolder).toHaveValue("/draft/two");
  });

  it("associates validation with the invalid control and never submits it", async () => {
    const invalid = settings({
      output: {
        default_mode: "suffix",
        suffix: "",
        separate_folder: null,
        overwrite_existing: false,
      },
    });
    const tauri = installTauriMock();
    await renderApp(<SettingsView />, { appState: { settings: invalid } });

    const suffix = page.getByRole("textbox", { name: "Filename suffix", exact: true });
    await expect.element(suffix).toHaveAttribute("aria-invalid", "true");
    await expect
      .element(page.getByText("A filename suffix is required in suffix mode."))
      .toBeVisible();
    await expect.element(page.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(tauri.callsFor("set_settings")).toHaveLength(0);
  });

  it("rejects a missing separate output folder at its control", async () => {
    const invalid = settings({
      output: {
        default_mode: "separate_folder",
        suffix: "_av1",
        separate_folder: null,
        overwrite_existing: false,
      },
    });
    const tauri = installTauriMock();
    await renderApp(<SettingsView />, { appState: { settings: invalid } });

    const folder = page.getByRole("textbox", { name: "Separate output folder" });
    await expect.element(folder).toHaveAttribute("aria-invalid", "true");
    await expect
      .element(page.getByText("An output folder is required in separate-folder mode."))
      .toBeVisible();
    await expect.element(page.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(tauri.callsFor("set_settings")).toHaveLength(0);
  });

  it("preserves the draft after a rejected save and accepts committed reconnect changes", async () => {
    const committed = settings();
    const tauri = installTauriMock();
    tauri.rejectCommand("set_settings", { code: "rejected", message: "settings are locked" });
    await renderApp(<SettingsView />, { appState: { settings: committed } });

    const inputFolder = page.getByRole("textbox", { name: "Input folder", exact: true });
    await inputFolder.fill("/keep/me");
    await page.getByRole("button", { name: "Save changes" }).click();
    await expect.element(inputFolder).toHaveValue("/keep/me");
    await expect
      .element(page.getByText("settings save failed (rejected): settings are locked"))
      .toBeVisible();
    await expect.element(page.getByRole("button", { name: "Save changes" })).toBeEnabled();

    const reconnected = { ...committed, last_input_folder: "/engine/value" };
    appStore.setState({ settings: reconnected });
    await expect.element(inputFolder).toHaveValue("/engine/value");
  });

  it("uses native pickers, preserves values on cancel, and imports the selected history", async () => {
    const tauri = installTauriMock({
      pick_paths: () => [],
      import_history: () => ({ parked: 4, skipped: 1 }),
    });
    await renderApp(<SettingsView />, { appState: { settings: settings() } });

    const inputFolder = page.getByRole("textbox", { name: "Input folder", exact: true });
    await page.getByRole("button", { name: "Choose input folder" }).click();
    await expect.element(inputFolder).toHaveValue("");

    tauri.setCommand("pick_paths", (payload) =>
      payload !== undefined &&
      !Array.isArray(payload) &&
      !(payload instanceof ArrayBuffer) &&
      !ArrayBuffer.isView(payload) &&
      "kind" in payload &&
      payload.kind === "HistoryImport"
        ? ["/exports/history.json"]
        : ["/videos"],
    );
    await page.getByRole("button", { name: "Choose input folder" }).click();
    await expect.element(inputFolder).toHaveValue("/videos");

    await page.getByRole("button", { name: "Choose history export" }).click();
    await expect
      .element(page.getByRole("textbox", { name: "Import history", exact: true }))
      .toHaveValue("/exports/history.json");
    await page.getByRole("button", { name: "Import", exact: true }).click();
    await expect.element(page.getByText("Parked 4, skipped 1")).toBeVisible();

    tauri.rejectCommand("import_history", { code: "import_failed", message: "invalid export" });
    await page.getByRole("button", { name: "Import", exact: true }).click();
    await expect
      .element(page.getByText("history import failed (import_failed): invalid export"))
      .toBeVisible();
  });

  it("exposes only valid vendor actions and follows streamed tool activity", async () => {
    const tauri = installTauriMock({ vendor_install: () => null, vendor_check: () => null });
    await renderApp(<SettingsView />, {
      appState: { settings: settings(), tools: missingTools() },
    });

    await page.getByRole("button", { name: "Install" }).click();
    expect(tauri.callsFor("vendor_install")).toHaveLength(1);

    appStore.setState({
      tools: { ...missingTools(), activity: { Downloading: { received: 512, total: 1024 } } },
    });
    await expect.element(page.getByText("Downloading dependencies…")).toBeVisible();
    await expect.element(page.getByRole("progressbar", { name: "Download" })).toBeVisible();
    await expect.element(page.getByText("512 B / 1.00 KB")).toBeVisible();
    await expect.element(page.getByRole("button", { name: "Check", exact: true })).toBeDisabled();

    appStore.setState({ tools: availableTools(true) });
    await expect.element(page.getByRole("button", { name: "Update", exact: true })).toBeEnabled();
  });

  it("exposes vendor actions only in valid streamed states and session conditions", async () => {
    installTauriMock({ vendor_install: () => null, vendor_check: () => null });
    await renderApp(<SettingsView />, { appState: { settings: settings(), tools: null } });

    await expect.element(page.getByRole("button", { name: "Check", exact: true })).toBeDisabled();
    await expect.element(page.getByRole("button", { name: "Install" })).not.toBeInTheDocument();

    appStore.setState({ tools: { ...missingTools(), activity: "Checking" } });
    await expect.element(page.getByRole("button", { name: "Checking…" })).toBeDisabled();
    await expect.element(page.getByRole("button", { name: "Install" })).toBeDisabled();

    appStore.setState({ tools: { ...missingTools(), activity: "Installing" } });
    await expect.element(page.getByRole("button", { name: "Check", exact: true })).toBeDisabled();

    appStore.setState({ tools: availableTools() });
    await expect.element(page.getByRole("button", { name: "Check", exact: true })).toBeEnabled();
    await expect.element(page.getByRole("button", { name: "Install" })).not.toBeInTheDocument();

    appStore.setState({
      tools: {
        ...missingTools(),
        activity: { Failed: { detail: "archive verification failed" } },
      },
    });
    await expect
      .element(
        page
          .getByText("Dependency operation failed: archive verification failed", {
            exact: true,
          })
          .last(),
      )
      .toBeVisible();
    await expect.element(page.getByRole("button", { name: "Retry install" })).toBeEnabled();

    appStore.setState({ session: "Running" });
    await expect.element(page.getByRole("button", { name: "Retry install" })).toBeDisabled();

    appStore.setState((state) => ({
      ...state,
      session: "Idle",
      durable: {
        ...state.durable,
        queue: [
          {
            id: 1,
            input: "/videos/active.mkv",
            operation: "Convert",
            intent: "ReuseIfFresh",
            output_target: "Replace",
            overwrite: "FollowSettings",
            state: { Reserved: { claim_id: 1, run_id: 1 } },
          },
        ],
      },
    }));
    await expect.element(page.getByRole("button", { name: "Retry install" })).toBeDisabled();
  });

  it("checks application updates and confirms irreversible log scrubbing", async () => {
    const tauri = installTauriMock({
      check_for_update: () => ({ current: "3.0.0", latest: "3.1.0", update_available: true }),
      open_release_page: () => null,
      scrub_logs: () => ({ total: 5, modified: 3, failed: 0 }),
    });
    await renderApp(<SettingsView />, { appState: { settings: settings() } });

    await page.getByRole("button", { name: "Check for updates" }).click();
    await expect
      .element(page.getByText("Version 3.1.0 is available; this app is 3.0.0."))
      .toBeVisible();
    await page.getByRole("button", { name: "Open release page" }).click();
    expect(tauri.callsFor("open_release_page")).toHaveLength(1);

    await page.getByRole("button", { name: "Scrub logs", exact: true }).first().click();
    const confirmation = page.getByRole("alertdialog");
    await expect.element(confirmation).toBeVisible();
    await confirmation.getByRole("button", { name: "Scrub logs" }).click();

    expect(tauri.callsFor("scrub_logs")).toHaveLength(1);
    await expect
      .element(page.getByText("Examined 5 log files; rewrote 3; 0 failed."))
      .toBeVisible();
  });

  it("announces current/update failures and prevents duplicate log scrubs", async () => {
    const scrub = deferred<{ total: number; modified: number; failed: number }>();
    const tauri = installTauriMock({
      check_for_update: () => ({ current: "3.0.0", latest: "3.0.0", update_available: false }),
      scrub_logs: () => scrub.promise,
    });
    await renderApp(<SettingsView />, { appState: { settings: settings() } });

    await page.getByRole("button", { name: "Check for updates" }).click();
    await expect
      .element(page.getByRole("status").getByText("Version 3.0.0 is current."))
      .toBeVisible();

    tauri.rejectCommand("check_for_update", { code: "network", message: "offline" });
    await page.getByRole("button", { name: "Check for updates" }).click();
    await expect
      .element(page.getByRole("alert").getByText("update check failed (network): offline"))
      .toBeVisible();

    await page.getByRole("button", { name: "Scrub logs", exact: true }).first().click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Scrub logs" }).click();
    await expect.element(page.getByRole("button", { name: "Scrubbing…" })).toBeDisabled();
    expect(tauri.callsFor("scrub_logs")).toHaveLength(1);

    scrub.resolve({ total: 2, modified: 1, failed: 0 });
    await expect
      .element(page.getByText("Examined 2 log files; rewrote 1; 0 failed."))
      .toBeVisible();

    tauri.rejectCommand("scrub_logs", { code: "scrub_failed", message: "file locked" });
    await page.getByRole("button", { name: "Scrub logs", exact: true }).first().click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Scrub logs" }).click();
    await expect
      .element(page.getByText("log scrub failed (scrub_failed): file locked"))
      .toBeVisible();
  });
});
