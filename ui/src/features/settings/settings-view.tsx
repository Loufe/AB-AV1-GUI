import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { DefaultOutputMode, Settings, VideoExtension } from "@/lib/bindings";
import { saveSettings } from "@/lib/ipc";

import { HistoryImport } from "./history-import";
import { SettingContainer, SettingsGroup } from "./settings-primitives";
import { useSettings } from "./use-settings";

const VIDEO_EXTENSIONS: readonly { value: VideoExtension; label: string }[] = [
  { value: "mp4", label: "MP4" },
  { value: "mkv", label: "MKV" },
  { value: "avi", label: "AVI" },
  { value: "wmv", label: "WMV" },
];

function settingsEqual(left: Settings, right: Settings): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function validationMessage(settings: Settings): string | null {
  if (settings.output.default_mode === "suffix" && settings.output.suffix.trim().length === 0) {
    return "A filename suffix is required in suffix mode.";
  }
  if (
    settings.output.default_mode === "separate_folder" &&
    (settings.output.separate_folder === null || settings.output.separate_folder.length === 0)
  ) {
    return "An output folder is required in separate-folder mode.";
  }
  return null;
}

function optionalPath(value: string): string | null {
  return value.length === 0 ? null : value;
}

export function SettingsView() {
  const committed = useSettings();
  const [draft, setDraft] = useState<Settings | null>(committed);
  const lastCommitted = useRef(committed);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    // Activity recreates effects when a hidden view becomes visible. Only a
    // real acknowledged settings change replaces the draft; navigation alone
    // must not discard unsaved edits.
    const previous = lastCommitted.current;
    if (previous === committed) {
      return;
    }
    lastCommitted.current = committed;
    if (previous !== null && committed !== null && settingsEqual(previous, committed)) {
      return;
    }
    setDraft(committed);
  }, [committed]);

  if (committed === null || draft === null) {
    return (
      <div className="mx-auto max-w-2xl p-6">
        <SettingsGroup title="Settings">
          <SettingContainer
            label="Waiting for the engine"
            description="Settings become available after the desktop engine sends its snapshot."
            last
          >
            <Button variant="outline" size="sm" disabled>
              Unavailable
            </Button>
          </SettingContainer>
        </SettingsGroup>
      </div>
    );
  }

  const validation = validationMessage(draft);
  const dirty = !settingsEqual(committed, draft);
  const disabled = saving;

  const updateOutputMode = (mode: DefaultOutputMode | null) => {
    if (mode !== null) {
      setDraft({ ...draft, output: { ...draft.output, default_mode: mode } });
    }
  };

  const toggleExtension = (extension: VideoExtension, checked: boolean) => {
    const selected = new Set(draft.scan_extensions);
    if (checked) {
      selected.add(extension);
    } else {
      selected.delete(extension);
    }
    setDraft({
      ...draft,
      scan_extensions: VIDEO_EXTENSIONS.map(({ value }) => value).filter((value) =>
        selected.has(value),
      ),
    });
  };

  const save = async () => {
    if (validation !== null) {
      toast.error(validation);
      return;
    }
    setSaving(true);
    try {
      await saveSettings(draft);
      toast.success("Settings saved");
    } catch (error: unknown) {
      toast.error(error instanceof Error ? error.message : "Settings could not be saved");
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-4 p-6">
      <SettingsGroup title="Files">
        <SettingContainer
          label="Input folder"
          description="The folder opened by default for analysis and queueing"
        >
          <Input
            className="w-72"
            value={draft.last_input_folder ?? ""}
            placeholder="No default folder"
            disabled={disabled}
            onChange={(event) =>
              setDraft({ ...draft, last_input_folder: optionalPath(event.target.value) })
            }
          />
        </SettingContainer>
        <SettingContainer
          label="Video extensions"
          description="File types included while scanning; all may be disabled"
          last
        >
          <div className="flex gap-3">
            {VIDEO_EXTENSIONS.map(({ value, label }) => (
              <label key={value} className="flex items-center gap-1.5 text-xs">
                <Checkbox
                  checked={draft.scan_extensions.includes(value)}
                  disabled={disabled}
                  onCheckedChange={(checked) => toggleExtension(value, checked)}
                />
                {label}
              </label>
            ))}
          </div>
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title="Conversion">
        <SettingContainer
          label="Hardware-accelerated decoding"
          description="Prefer the GPU during quality sampling, with automatic software fallback"
          last
        >
          <Switch
            checked={draft.hardware_decode}
            disabled={disabled}
            onCheckedChange={(checked) => setDraft({ ...draft, hardware_decode: checked })}
          />
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title="Output">
        <SettingContainer label="Default output mode" description="Used for newly queued files">
          <Select
            value={draft.output.default_mode}
            disabled={disabled}
            onValueChange={updateOutputMode}
          >
            <SelectTrigger size="sm" className="w-44">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="replace">Replace original</SelectItem>
              <SelectItem value="suffix">Save with suffix</SelectItem>
              <SelectItem value="separate_folder">Separate folder</SelectItem>
            </SelectContent>
          </Select>
        </SettingContainer>
        <SettingContainer
          label="Filename suffix"
          description="Remembered while another output mode is active"
        >
          <Input
            className="w-44"
            value={draft.output.suffix}
            disabled={disabled || draft.output.default_mode !== "suffix"}
            aria-invalid={draft.output.default_mode === "suffix" && validation !== null}
            onChange={(event) =>
              setDraft({ ...draft, output: { ...draft.output, suffix: event.target.value } })
            }
          />
        </SettingContainer>
        <SettingContainer
          label="Separate output folder"
          description="Remembered while another output mode is active"
        >
          <Input
            className="w-72"
            value={draft.output.separate_folder ?? ""}
            placeholder="No folder selected"
            disabled={disabled || draft.output.default_mode !== "separate_folder"}
            aria-invalid={draft.output.default_mode === "separate_folder" && validation !== null}
            onChange={(event) =>
              setDraft({
                ...draft,
                output: {
                  ...draft.output,
                  separate_folder: optionalPath(event.target.value),
                },
              })
            }
          />
        </SettingContainer>
        <SettingContainer
          label="Overwrite existing outputs"
          description="Allow a conversion to replace an existing destination file"
          last
        >
          <Switch
            checked={draft.output.overwrite_existing}
            disabled={disabled}
            onCheckedChange={(checked) =>
              setDraft({
                ...draft,
                output: { ...draft.output, overwrite_existing: checked },
              })
            }
          />
        </SettingContainer>
      </SettingsGroup>

      <SettingsGroup title="Privacy and logs">
        <SettingContainer
          label="Anonymize paths in logs"
          description="Hash file and folder names written to diagnostic logs"
        >
          <Switch
            checked={draft.privacy.anonymize_logs}
            disabled={disabled}
            onCheckedChange={(checked) =>
              setDraft({
                ...draft,
                privacy: { ...draft.privacy, anonymize_logs: checked },
              })
            }
          />
        </SettingContainer>
        <SettingContainer
          label="Anonymize paths in history"
          description="Store hashed paths in conversion history"
        >
          <Switch
            checked={draft.privacy.anonymize_history}
            disabled={disabled}
            onCheckedChange={(checked) =>
              setDraft({
                ...draft,
                privacy: { ...draft.privacy, anonymize_history: checked },
              })
            }
          />
        </SettingContainer>
        <SettingContainer
          label="Log folder"
          description="Leave empty to use the application default"
          last
        >
          <Input
            className="w-72"
            value={draft.log_folder ?? ""}
            placeholder="Application default"
            disabled={disabled}
            onChange={(event) =>
              setDraft({ ...draft, log_folder: optionalPath(event.target.value) })
            }
          />
        </SettingContainer>
      </SettingsGroup>

      <HistoryImport />

      <div className="flex items-center justify-end gap-3">
        {validation !== null && <p className="mr-auto text-xs text-destructive">{validation}</p>}
        <Button variant="outline" disabled={disabled || !dirty} onClick={() => setDraft(committed)}>
          Reset
        </Button>
        <Button disabled={disabled || !dirty || validation !== null} onClick={save}>
          {saving ? "Saving…" : "Save changes"}
        </Button>
      </div>
    </div>
  );
}
