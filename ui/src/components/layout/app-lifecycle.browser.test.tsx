import { page, userEvent } from "vitest/browser";
import { describe, expect, it } from "vitest";

import App from "@/App";
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
});
