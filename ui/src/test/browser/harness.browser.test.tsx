import { Activity, useEffect, useState } from "react";
import { page, userEvent } from "vitest/browser";
import { describe, expect, it } from "vitest";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAppStore } from "@/lib/store/app-store";

import { renderApp } from "./render";

function SessionProbe() {
  const session = useAppStore((state) => state.session);
  return <output aria-label="Current session">{session}</output>;
}

function ActionsMenu() {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger render={<Button>File actions</Button>} />
      <DropdownMenuContent>
        <DropdownMenuItem>Open file</DropdownMenuItem>
        <DropdownMenuItem>Reveal in folder</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

interface LifecycleProbe {
  active: boolean;
  starts: number;
  cleanups: number;
}

function StatefulChild({ lifecycle }: { lifecycle: LifecycleProbe }) {
  const [count, setCount] = useState(0);

  useEffect(() => {
    lifecycle.active = true;
    lifecycle.starts += 1;
    return () => {
      lifecycle.active = false;
      lifecycle.cleanups += 1;
    };
  }, [lifecycle]);

  return <Button onClick={() => setCount((current) => current + 1)}>Count {count}</Button>;
}

function ActivityFixture({ lifecycle }: { lifecycle: LifecycleProbe }) {
  const [visible, setVisible] = useState(true);
  return (
    <>
      <Button onClick={() => setVisible((current) => !current)}>
        {visible ? "Hide panel" : "Show panel"}
      </Button>
      <Activity mode={visible ? "visible" : "hidden"}>
        <StatefulChild lifecycle={lifecycle} />
      </Activity>
    </>
  );
}

describe("browser render harness", () => {
  it("starts every render from explicitly isolated Zustand state", async () => {
    const first = await renderApp(<SessionProbe />, { appState: { session: "Running" } });
    await expect.element(page.getByLabelText("Current session")).toHaveTextContent("Running");
    await first.unmount();

    await renderApp(<SessionProbe />);
    await expect.element(page.getByLabelText("Current session")).toHaveTextContent("Idle");
  });

  it("drives a portalled Base UI menu through keyboard roles and restores focus", async () => {
    const rendered = await renderApp(<ActionsMenu />);
    const trigger = page.getByRole("button", { name: "File actions" });

    await userEvent.tab();
    await expect.element(trigger).toHaveFocus();
    await userEvent.keyboard("{Enter}");

    const menu = page.getByRole("menu");
    await expect.element(menu).toBeVisible();
    expect(rendered.container.contains(menu.element())).toBe(false);

    await expect.element(page.getByRole("menuitem", { name: "Open file" })).toHaveFocus();
    await userEvent.keyboard("{ArrowDown}");
    await expect.element(page.getByRole("menuitem", { name: "Reveal in folder" })).toHaveFocus();

    await userEvent.keyboard("{Escape}");
    await expect.element(menu).not.toBeInTheDocument();
    await expect.element(trigger).toHaveFocus();
  });

  it("preserves local state while Activity cleans up and restarts hidden effects", async () => {
    const lifecycle: LifecycleProbe = { active: false, starts: 0, cleanups: 0 };
    await renderApp(<ActivityFixture lifecycle={lifecycle} />);

    const counter = page.getByRole("button", { name: "Count 0" });
    await expect.poll(() => lifecycle.active).toBe(true);
    await counter.click();
    await expect.element(page.getByRole("button", { name: "Count 1" })).toBeVisible();

    const startsBeforeHide = lifecycle.starts;
    await page.getByRole("button", { name: "Hide panel" }).click();
    await expect.element(page.getByText("Count 1", { exact: true })).not.toBeVisible();
    await expect.poll(() => lifecycle.active).toBe(false);
    expect(lifecycle.cleanups).toBeGreaterThan(0);

    await page.getByRole("button", { name: "Show panel" }).click();
    await expect.element(page.getByRole("button", { name: "Count 1" })).toBeVisible();
    await expect.poll(() => lifecycle.active).toBe(true);
    expect(lifecycle.starts).toBeGreaterThan(startsBeforeHide);
  });
});
