import { page, userEvent } from "vitest/browser";
import { describe, expect, it } from "vitest";

import type { DurableState_Deserialize, QueueItem, Settings, ToolsState } from "@/lib/bindings";
import { appStore } from "@/lib/store/app-store";
import { emptyDurableState } from "@/lib/store/fold";
import { renderApp } from "@/test/browser/render";
import { installTauriMock } from "@/test/browser/tauri";

import { QueueView } from "./queue-view";

function settings(): Settings {
  return {
    last_input_folder: "C:\\Videos",
    scan_extensions: ["mp4", "mkv"],
    output: {
      default_mode: "suffix",
      suffix: "_av1",
      separate_folder: "D:\\Encoded",
      overwrite_existing: false,
    },
    hardware_decode: true,
    privacy: { anonymize_logs: false, anonymize_history: false },
    log_folder: null,
  };
}

function tools(): ToolsState {
  return {
    availability: {
      Available: {
        source: "System",
        revisions: { ab_av1: "1", ffmpeg: "2", encoder: "3" },
      },
    },
    activity: "Idle",
    update_available: false,
  };
}

function item(
  id: number,
  name: string,
  state: QueueItem["state"] = "Queued",
  folder = "Videos",
): QueueItem {
  return {
    id,
    input: `C:\\${folder}\\${name}`,
    operation: "Convert",
    intent: "ReuseIfFresh",
    output_target: { Suffix: { suffix: "_av1" } },
    overwrite: "FollowSettings",
    state,
  };
}

function durable(queue: QueueItem[]): DurableState_Deserialize {
  return { ...emptyDurableState(), queue };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

function replaceQueue(queue: QueueItem[]): void {
  appStore.setState((state) => ({ ...state, durable: durable(queue) }));
}

describe("QueueView interaction protocol", () => {
  it("waits for delta after an early acknowledgement and sends nothing for a no-op", async () => {
    const tauri = installTauriMock({ queue_reorder_pending: () => null });
    const initial = [item(1, "one.mkv"), item(2, "two.mkv"), item(3, "three.mkv")];
    await renderApp(<QueueView />, {
      appState: { durable: durable(initial), settings: settings(), tools: tools() },
    });

    await page.getByRole("button", { name: "Group by folder" }).click();
    const firstHandle = page.getByRole("button", { name: "Reorder one.mkv" });
    firstHandle.element().focus();
    await userEvent.keyboard(" ");
    await userEvent.keyboard(" ");
    expect(tauri.callsFor("queue_reorder_pending")).toHaveLength(0);
    await expect
      .element(page.getByRole("status").filter({ hasText: "Queue order unchanged" }))
      .toBeInTheDocument();

    firstHandle.element().focus();
    await userEvent.keyboard(" ");
    appStore.setState((state) => ({
      ...state,
      snapshotGeneration: state.snapshotGeneration + 1,
    }));
    await expect
      .element(page.getByRole("status").filter({ hasText: "snapshot changed; cancelled moving" }))
      .toBeInTheDocument();
    expect(tauri.callsFor("queue_reorder_pending")).toHaveLength(0);

    await page.getByRole("row", { name: /two\.mkv/ }).click();
    await page.getByRole("button", { name: "Move selected to top" }).click();
    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(1);
    await expect
      .element(page.getByRole("status").filter({ hasText: "Submitting Queue order" }))
      .toBeInTheDocument();
    expect(appStore.getState().durable.queue.map((entry) => entry.id)).toEqual([1, 2, 3]);

    replaceQueue([initial[1]!, initial[0]!, initial[2]!]);
    await expect
      .element(page.getByRole("status").filter({ hasText: "Moved two.mkv to position 1 of 3" }))
      .toBeInTheDocument();
  });

  it("uses real pointer dragging for a stable selected block and a folder run", async () => {
    const reorder = deferred<null>();
    const tauri = installTauriMock({ queue_reorder_pending: () => reorder.promise });
    const rows = [
      item(1, "same.mkv", "Queued", "A"),
      item(2, "same.mkv", "Queued", "B"),
      item(3, "last.mkv", "Queued", "C"),
    ];
    await renderApp(<QueueView />, {
      appState: { durable: durable(rows), settings: settings(), tools: tools() },
    });

    await page.getByRole("button", { name: "Group by folder" }).click();
    const firstRow = page.getByRole("row", { name: /same\.mkv/ }).first();
    const lastRow = page.getByRole("row", { name: /last\.mkv/ });
    await firstRow.click();
    lastRow.element().dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    await userEvent.dragAndDrop(
      page.getByRole("button", { name: "Reorder last.mkv" }),
      page.getByRole("button", { name: "Reorder same.mkv" }).nth(1),
    );
    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(1);
    expect(tauri.callsFor("queue_reorder_pending")[0]?.payload).toMatchObject({
      pendingOrder: [1, 3, 2],
    });

    reorder.resolve(null);
    replaceQueue([rows[0]!, rows[2]!, rows[1]!]);
    await expect.element(page.getByRole("button", { name: "Group by folder" })).toBeEnabled();
    await page.getByRole("button", { name: "Group by folder" }).click();

    const folderReorder = deferred<null>();
    tauri.setCommand("queue_reorder_pending", () => folderReorder.promise);
    await userEvent.dragAndDrop(
      page.getByRole("button", { name: "Reorder folder B" }),
      page.getByRole("button", { name: "Reorder folder A" }),
    );
    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(2);
    expect(tauri.callsFor("queue_reorder_pending")[1]?.payload).toMatchObject({
      pendingOrder: [2, 1, 3],
    });
    folderReorder.resolve(null);
  });

  it("cancels an immutable cross-folder confirmation when its baseline changes", async () => {
    const tauri = installTauriMock({ queue_reorder_pending: () => null });
    const rows = [
      item(1, "one.mkv", "Queued", "A"),
      item(2, "two.mkv", "Queued", "A"),
      item(3, "three.mkv", "Queued", "B"),
    ];
    await renderApp(<QueueView />, {
      appState: { durable: durable(rows), settings: settings(), tools: tools() },
    });

    await page.getByRole("row", { name: /two\.mkv/ }).click();
    await page.getByRole("button", { name: "Move selected down" }).click();
    await expect.element(page.getByRole("alertdialog")).toBeVisible();

    replaceQueue([rows[1]!, rows[0]!, rows[2]!]);
    await expect.element(page.getByRole("alertdialog")).not.toBeInTheDocument();
    await expect.element(page.getByRole("alert")).toHaveTextContent("latest order was restored");
    expect(tauri.callsFor("queue_reorder_pending")).toHaveLength(0);
    await expect
      .poll(() => document.activeElement?.getAttribute("aria-label"))
      .toBe("Move selected down");
  });

  it("keeps repeated folder runs distinct and excludes frozen rows from reorder payloads", async () => {
    const tauri = installTauriMock({ queue_reorder_pending: () => null });
    const frozen = item(4, "frozen.mkv", { Finished: "Analyzed" }, "A");
    await renderApp(<QueueView />, {
      appState: {
        durable: durable([
          item(1, "same.mkv", "Queued", "A"),
          item(2, "same.mkv", "Queued", "B"),
          frozen,
          item(3, "last.mkv", "Queued", "A"),
        ]),
        settings: settings(),
        tools: tools(),
      },
    });

    const repeatedHandles = page.getByRole("button", { name: "Reorder folder A" }).elements();
    expect(repeatedHandles).toHaveLength(2);
    expect(repeatedHandles[0]?.dataset.queueFolderHandle).not.toBe(
      repeatedHandles[1]?.dataset.queueFolderHandle,
    );
    expect(page.getByRole("button", { name: "Reorder frozen.mkv" }).elements()).toHaveLength(0);

    await page.getByRole("button", { name: "Move A bottom" }).first().click();
    await expect.poll(() => tauri.callsFor("queue_reorder_pending").length).toBe(1);
    expect(tauri.callsFor("queue_reorder_pending")[0]?.payload).toMatchObject({
      pendingOrder: [2, 3, 1],
    });
  });

  it("removes multiple items with one ordered payload and rejects mixed active selection", async () => {
    const tauri = installTauriMock();
    tauri.rejectCommand("queue_remove_many", { code: "rejected", message: "queue changed" });
    const rows = [item(1, "one.mkv"), item(2, "two.mkv"), item(3, "three.mkv")];
    await renderApp(<QueueView />, {
      appState: { durable: durable(rows), settings: settings(), tools: tools() },
    });

    await page.getByRole("row", { name: /three\.mkv/ }).click();
    page
      .getByRole("row", { name: /one\.mkv/ })
      .element()
      .dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    await page.getByRole("button", { name: "Remove 2" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Remove 2" }).click();
    await expect.poll(() => tauri.callsFor("queue_remove_many").length).toBe(1);
    expect(tauri.callsFor("queue_remove_many")[0]?.payload).toMatchObject({ itemIds: [1, 3] });
    await expect.element(page.getByRole("alert")).toHaveTextContent("queue changed");
    expect(appStore.getState().durable.queue.map((entry) => entry.id)).toEqual([1, 2, 3]);

    const active = item(4, "active.mkv", { Running: { claim_id: 4, run_id: 4 } });
    replaceQueue([active, rows[0]!]);
    await page.getByRole("row", { name: /active\.mkv/ }).click();
    page
      .getByRole("row", { name: /one\.mkv/ })
      .element()
      .dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    await expect.element(page.getByRole("button", { name: "Remove 2" })).toBeDisabled();
    expect(tauri.callsFor("queue_remove_many")).toHaveLength(1);
  });

  it("restores authoritative text after empty or rejected output edits", async () => {
    const tauri = installTauriMock();
    tauri.rejectCommand("queue_edit", { code: "rejected", message: "invalid output target" });
    const suffixItem = item(8, "edit.mkv");
    await renderApp(<QueueView />, {
      appState: { durable: durable([suffixItem]), settings: settings(), tools: tools() },
    });
    await page.getByRole("row", { name: /edit\.mkv/ }).click();

    const suffix = page.getByRole("textbox", { name: "Output suffix" });
    await userEvent.fill(suffix, "../escape");
    await userEvent.tab();
    await expect.element(page.getByRole("alert")).toHaveTextContent("invalid output target");
    await expect.element(suffix).toHaveValue("_av1");

    const folderItem: QueueItem = {
      ...suffixItem,
      output_target: {
        SeparateFolder: { directory: "D:\\Old", source_root: "C:\\Videos" },
      },
    };
    replaceQueue([folderItem]);
    const folder = page.getByRole("textbox", { name: "Output folder" });
    await userEvent.fill(folder, "D:\\Rejected");
    await userEvent.tab();
    await expect.element(folder).toHaveValue("D:\\Old");

    await userEvent.fill(folder, "   ");
    await userEvent.tab();
    await expect.element(folder).toHaveValue("D:\\Old");
  });

  it("preserves output facts across Analyze and edits every tri-state recovery choice", async () => {
    const tauri = installTauriMock({
      queue_edit: () => null,
      queue_retry: () => null,
    });
    tauri.rejectCommand("reveal_in_file_manager", {
      code: "io",
      message: "cannot reveal file",
    });
    const editable: QueueItem = {
      ...item(8, "edit.mkv"),
      output_target: {
        SeparateFolder: { directory: "D:\\Old", source_root: "C:\\Videos" },
      },
    };
    await renderApp(<QueueView />, {
      appState: { durable: durable([editable]), settings: settings(), tools: tools() },
    });
    await page.getByRole("row", { name: /edit\.mkv/ }).click();

    await page.getByRole("combobox", { name: "Operation" }).click();
    await page.getByRole("option", { name: "Analyze" }).click();
    await expect.poll(() => tauri.callsFor("queue_edit").length).toBe(1);
    expect(tauri.callsFor("queue_edit")[0]?.payload).toMatchObject({
      itemId: 8,
      patch: { operation: "Analyze" },
    });
    replaceQueue([{ ...editable, operation: "Analyze" }]);
    await expect
      .element(page.getByRole("combobox", { name: "Output target" }))
      .not.toBeInTheDocument();
    await expect.element(page.getByText("Old", { exact: true })).toBeVisible();

    await page.getByRole("combobox", { name: "Operation" }).click();
    await page.getByRole("option", { name: "Convert" }).click();
    replaceQueue([editable]);
    await expect.element(page.getByRole("combobox", { name: "Output target" })).toBeVisible();
    await expect
      .element(page.getByRole("textbox", { name: "Output folder" }))
      .toHaveValue("D:\\Old");

    for (const [label, overwrite] of [
      ["Allow", "Allow"],
      ["Deny", "Deny"],
      ["Follow Settings", "FollowSettings"],
    ] as const) {
      await page.getByRole("combobox", { name: "Overwrite decision" }).click();
      await page.getByRole("option", { name: label }).click();
      await expect
        .poll(() => tauri.callsFor("queue_edit").at(-1)?.payload)
        .toMatchObject({ itemId: 8, patch: { overwrite } });
      replaceQueue([{ ...editable, overwrite }]);
    }

    await userEvent.fill(page.getByRole("textbox", { name: "Output folder" }), "D:\\New");
    await userEvent.tab();
    await expect
      .poll(() => tauri.callsFor("queue_edit").at(-1)?.payload)
      .toMatchObject({
        itemId: 8,
        patch: {
          output_target: {
            SeparateFolder: { directory: "D:\\New", source_root: "C:\\Videos" },
          },
        },
      });

    const finished = { ...editable, state: { Finished: "Stopped" } } satisfies QueueItem;
    replaceQueue([finished]);
    await page.getByRole("button", { name: "Re-analyze" }).click();
    await expect
      .poll(() => tauri.callsFor("queue_retry").at(-1)?.payload)
      .toMatchObject({ itemId: 8, patch: { operation: "Analyze", intent: "Refresh" } });

    replaceQueue([{ ...editable, operation: "Analyze", intent: "Refresh", state: "Queued" }]);
    await expect.element(page.getByText("Refresh", { exact: true }).first()).toBeVisible();

    await page.getByRole("button", { name: "Reveal" }).click();
    await expect.element(page.getByRole("alert")).toHaveTextContent("cannot reveal file");
  });
});
