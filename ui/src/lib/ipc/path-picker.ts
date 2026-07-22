import { commands, type PathPickerKind } from "@/lib/bindings";

/** Open the shell-owned native picker; cancellation is an accepted empty list. */
export async function pickPaths(
  kind: PathPickerKind,
  startingDirectory: string | null = null,
): Promise<string[]> {
  const result = await commands.pickPaths(kind, startingDirectory);
  if (result.status === "error") {
    throw new Error(`path picker failed (${result.error.code}): ${result.error.message}`);
  }
  return result.data;
}

export async function pickPath(
  kind: Extract<PathPickerKind, "File" | "Folder" | "HistoryImport">,
  startingDirectory: string | null = null,
): Promise<string | null> {
  return (await pickPaths(kind, startingDirectory))[0] ?? null;
}
