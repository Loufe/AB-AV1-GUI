import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { pickPath } from "@/lib/ipc/path-picker";

interface FolderInputProps {
  id: string;
  /** Which native picker Browse opens; the text field accepts either. */
  kind?: "File" | "Folder";
  value: string;
  placeholder: string;
  browseLabel: string;
  disabled?: boolean;
  invalid?: boolean;
  describedBy?: string;
  onChange: (value: string) => void;
}

export function FolderInput({
  id,
  kind = "Folder",
  value,
  placeholder,
  browseLabel,
  disabled = false,
  invalid = false,
  describedBy,
  onChange,
}: FolderInputProps) {
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const browse = async () => {
    setPicking(true);
    setError(null);
    try {
      const selected = await pickPath(kind, kind === "Folder" ? value || null : null);
      if (selected !== null) onChange(selected);
    } catch (pickerError: unknown) {
      setError(pickerError instanceof Error ? pickerError.message : "Path picker failed");
    } finally {
      setPicking(false);
    }
  };

  const errorId = `${id}-picker-error`;
  const ariaDescription = [describedBy, error === null ? null : errorId].filter(Boolean).join(" ");

  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex gap-2">
        <Input
          id={id}
          className="w-72"
          value={value}
          placeholder={placeholder}
          disabled={disabled || picking}
          aria-invalid={invalid}
          aria-describedby={ariaDescription || undefined}
          onChange={(event) => onChange(event.target.value)}
        />
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={disabled || picking}
          aria-label={browseLabel}
          onClick={() => void browse()}
        >
          {picking ? "Choosing…" : "Browse…"}
        </Button>
      </div>
      {error !== null && (
        <p id={errorId} className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
