import { useState } from "react";
import { page } from "vitest/browser";
import { describe, expect, it } from "vitest";

import { Button } from "@/components/ui/button";
import type { ShellEvent } from "@/lib/bindings";
import { queueClear, subscribeStream } from "@/lib/ipc";

import { renderApp } from "./render";
import { installTauriMock } from "./tauri";

function ClearQueueButton() {
  const [outcome, setOutcome] = useState("Ready");

  const clear = async () => {
    setOutcome("Clearing");
    try {
      await queueClear();
      setOutcome("Queue cleared");
    } catch (error: unknown) {
      setOutcome(error instanceof Error ? error.message : "Queue clear failed");
    }
  };

  return (
    <>
      <Button onClick={() => void clear()}>Clear queue</Button>
      <output aria-label="Command result">{outcome}</output>
    </>
  );
}

describe("Tauri browser boundary", () => {
  it("delivers generated-command success through the real wrapper", async () => {
    const tauri = installTauriMock({ queue_clear: () => null });
    await renderApp(<ClearQueueButton />);

    await page.getByRole("button", { name: "Clear queue" }).click();

    await expect.element(page.getByLabelText("Command result")).toHaveTextContent("Queue cleared");
    expect(tauri.callsFor("queue_clear")).toHaveLength(1);
  });

  it("delivers a generated-command rejection through the same wrapper", async () => {
    const tauri = installTauriMock();
    tauri.rejectCommand("queue_clear", { code: "rejected", message: "queue is running" });
    await renderApp(<ClearQueueButton />);

    await page.getByRole("button", { name: "Clear queue" }).click();

    await expect
      .element(page.getByLabelText("Command result"))
      .toHaveTextContent("queue clear failed (rejected): queue is running");
  });

  it("drives a subscribed generated channel with deterministic sequence numbers", async () => {
    const tauri = installTauriMock();
    const received: ShellEvent[] = [];
    await subscribeStream((event) => received.push(event));

    const first = tauri.emit("AbnormalShutdown");
    const second = tauri.emit("CloseRequested");

    expect(first.seq).toBe(0);
    expect(second.seq).toBe(1);
    expect(received).toEqual([first, second]);
    expect(tauri.callsFor("subscribe")).toHaveLength(1);
  });
});
