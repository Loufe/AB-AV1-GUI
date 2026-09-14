import { useState } from "react";

import { Button } from "@/components/ui/button";
import type { ToolPathSettings } from "@/lib/bindings";
import { recheckTools } from "@/lib/ipc/settings";
import { useAppStore } from "@/lib/store/app-store";
import { installGuidance, toolsStatus, type ToolsTone } from "@/lib/tools";

import { FolderInput } from "./path-input";
import { SettingContainer, SettingsGroup } from "./settings-primitives";

export type ToolPathField = "tool-ffmpeg" | "tool-ffprobe";

interface MediaToolsGroupProps {
  draft: ToolPathSettings;
  disabled: boolean;
  invalidField: ToolPathField | null;
  onChange: (tools: ToolPathSettings) => void;
}

const TONE_CLASS: Record<ToolsTone, string> = {
  muted: "text-muted-foreground",
  success: "text-success",
  warning: "text-warning",
  destructive: "text-destructive",
};

function optionalPath(value: string): string | null {
  return value.length === 0 ? null : value;
}

/**
 * Settings paths are the middle discovery tier: the CRFTY_FFMPEG and
 * CRFTY_FFPROBE environment overrides win over them, and an empty field
 * falls back to PATH. The status below is the engine's report, replayed on
 * every reconnect, so the panel never guesses at what is installed.
 */
export function MediaToolsGroup({ draft, disabled, invalidField, onChange }: MediaToolsGroupProps) {
  const tools = useAppStore((state) => state.tools);
  const platform = useAppStore((state) => state.platform);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const status = toolsStatus(tools);

  const recheck = async () => {
    setChecking(true);
    setError(null);
    try {
      await recheckTools();
    } catch (checkError: unknown) {
      setError(checkError instanceof Error ? checkError.message : "Tool check failed");
    } finally {
      setChecking(false);
    }
  };

  return (
    <SettingsGroup title="Media tools">
      <SettingContainer
        label="ffmpeg path"
        description="Leave empty to use ffmpeg from PATH"
        htmlFor="settings-ffmpeg-path"
      >
        <FolderInput
          id="settings-ffmpeg-path"
          kind="File"
          value={draft.ffmpeg ?? ""}
          placeholder="ffmpeg from PATH"
          browseLabel="Choose ffmpeg executable"
          disabled={disabled}
          invalid={invalidField === "tool-ffmpeg"}
          describedBy={invalidField === "tool-ffmpeg" ? "settings-error" : undefined}
          onChange={(value) => onChange({ ...draft, ffmpeg: optionalPath(value) })}
        />
      </SettingContainer>
      <SettingContainer
        label="ffprobe path"
        description="Leave empty to use ffprobe from PATH"
        htmlFor="settings-ffprobe-path"
      >
        <FolderInput
          id="settings-ffprobe-path"
          kind="File"
          value={draft.ffprobe ?? ""}
          placeholder="ffprobe from PATH"
          browseLabel="Choose ffprobe executable"
          disabled={disabled}
          invalid={invalidField === "tool-ffprobe"}
          describedBy={invalidField === "tool-ffprobe" ? "settings-error" : undefined}
          onChange={(value) => onChange({ ...draft, ffprobe: optionalPath(value) })}
        />
      </SettingContainer>
      <SettingContainer
        label="Tool status"
        description="Saved paths are checked immediately; the encoder and quality filter are verified when the Queue starts"
        last
      >
        <Button
          variant="outline"
          size="sm"
          disabled={checking || tools === null}
          onClick={() => void recheck()}
        >
          {checking ? "Checking…" : "Check again"}
        </Button>
      </SettingContainer>
      <div className="flex flex-col gap-1 border-t border-border px-4 py-2 text-xs">
        <p
          className={TONE_CLASS[status.tone]}
          role={status.tone === "destructive" ? "alert" : "status"}
        >
          {status.headline}
        </p>
        {status.details.map((detail) => (
          <p key={detail} className="break-all text-muted-foreground">
            {detail}
          </p>
        ))}
        {status.needsInstall &&
          installGuidance(platform).map((line) => (
            <p key={line} className="text-muted-foreground">
              {line}
            </p>
          ))}
        {error !== null && (
          <p className="text-destructive" role="alert">
            {error}
          </p>
        )}
      </div>
    </SettingsGroup>
  );
}
