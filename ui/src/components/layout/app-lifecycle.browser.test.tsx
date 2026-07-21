import { Activity, useState } from "react";
import { page, userEvent } from "vitest/browser";
import { describe, expect, it, vi } from "vitest";

import App from "@/App";
import { ErrorBoundary } from "@/components/error-boundary";
import { Button } from "@/components/ui/button";
import type { Settings } from "@/lib/bindings";
import { renderApp } from "@/test/browser/render";

function settings(): Settings {
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
    privacy: {
      anonymize_logs: false,
      anonymize_history: false,
    },
    log_folder: null,
  };
}

interface FailureGate {
  armed: boolean;
}

function CrashableView({ failure }: { failure: FailureGate }) {
  const [revision, setRevision] = useState(0);
  if (failure.armed) {
    throw new Error("view A failed");
  }
  return (
    <Button
      onClick={() => {
        failure.armed = true;
        setRevision((current) => current + 1);
      }}
    >
      Crash view A ({revision})
    </Button>
  );
}

function ErrorIsolationFixture({ failure }: { failure: FailureGate }) {
  const [active, setActive] = useState<"a" | "b">("a");
  const [viewBCount, setViewBCount] = useState(0);
  return (
    <>
      <Button onClick={() => setActive("a")}>Show view A</Button>
      <Button onClick={() => setActive("b")}>Show view B</Button>
      <Button onClick={() => (failure.armed = false)}>Repair view A</Button>
      <Activity mode={active === "a" ? "visible" : "hidden"}>
        <ErrorBoundary label="A test view">
          <CrashableView failure={failure} />
        </ErrorBoundary>
      </Activity>
      <Activity mode={active === "b" ? "visible" : "hidden"}>
        <ErrorBoundary label="B test view">
          <Button onClick={() => setViewBCount((current) => current + 1)}>
            View B count {viewBCount}
          </Button>
        </ErrorBoundary>
      </Activity>
    </>
  );
}

describe("application view lifecycle", () => {
  it("retains a visited view's draft and scroll while hidden controls leave navigation", async () => {
    await renderApp(<App />, { appState: { settings: settings() } });

    expect(document.querySelector("#view-panel-settings")).toBeNull();
    await page.getByRole("button", { name: "Settings" }).click();

    const inputFolder = page.getByPlaceholder("No default folder");
    await inputFolder.fill("/unsaved/draft");
    const settingsPanel = document.querySelector<HTMLElement>("#view-panel-settings");
    expect(settingsPanel).not.toBeNull();
    if (settingsPanel === null) return;
    expect(settingsPanel.scrollHeight).toBeGreaterThan(settingsPanel.clientHeight);
    settingsPanel.scrollTop = 120;
    expect(settingsPanel.scrollTop).toBe(120);

    const historyNav = page.getByRole("button", { name: "History" });
    await historyNav.click();

    await expect.element(historyNav).toHaveFocus();
    await expect.element(page.getByText("No records yet")).toBeVisible();
    await expect.element(inputFolder).not.toBeVisible();
    expect(document.querySelector("#view-panel-settings")).toBe(settingsPanel);

    await userEvent.tab();
    await expect.element(page.getByRole("button", { name: "Statistics" })).toHaveFocus();

    await page.getByRole("button", { name: "Settings" }).click();
    await expect.element(inputFolder).toHaveValue("/unsaved/draft");
    expect(settingsPanel.scrollTop).toBe(120);
  });

  it("mounts production views on first visit but unmounts the dev workshop", async () => {
    await renderApp(<App />, { appState: { settings: settings() } });

    expect(document.querySelector("#view-panel-history")).toBeNull();
    await page.getByRole("button", { name: "History" }).click();
    const historyPanel = document.querySelector("#view-panel-history");
    expect(historyPanel).not.toBeNull();

    await page.getByRole("button", { name: "Queue", exact: true }).click();
    expect(document.querySelector("#view-panel-history")).toBe(historyPanel);
    await expect.element(page.getByText("No records yet")).not.toBeVisible();

    await page.getByRole("button", { name: "Kitchen sink" }).click();
    await expect.element(page.getByRole("heading", { name: "Kitchen sink" })).toBeVisible();

    await page.getByRole("button", { name: "Queue", exact: true }).click();
    await expect
      .element(page.getByRole("heading", { name: "Kitchen sink" }))
      .not.toBeInTheDocument();
  });

  it("isolates a retained view crash without resetting another view", async () => {
    const failure: FailureGate = { armed: false };
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      await renderApp(<ErrorIsolationFixture failure={failure} />, {
        createRootOptions: { onCaughtError: () => undefined },
      });

      await page.getByRole("button", { name: "Crash view A (0)" }).click();
      await expect
        .element(page.getByText("Something went wrong in the A test view."))
        .toBeVisible();

      await page.getByRole("button", { name: "Show view B" }).click();
      await page.getByRole("button", { name: "View B count 0" }).click();
      await expect.element(page.getByRole("button", { name: "View B count 1" })).toBeVisible();

      await page.getByRole("button", { name: "Show view A" }).click();
      await page.getByRole("button", { name: "Repair view A" }).click();
      await page.getByRole("button", { name: "Reload view" }).click();
      await expect.element(page.getByRole("button", { name: "Crash view A (0)" })).toBeVisible();

      await page.getByRole("button", { name: "Show view B" }).click();
      await expect.element(page.getByRole("button", { name: "View B count 1" })).toBeVisible();
    } finally {
      consoleError.mockRestore();
    }
  });
});
