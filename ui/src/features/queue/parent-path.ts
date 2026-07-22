/** Browser-safe parent identity for a native path. The key is always a raw input prefix. */
export interface ParentPath {
  key: string;
  kind: "current" | "root" | "drive-relative" | "path";
  label: string;
}

const CURRENT_FOLDER_LABEL = "Current folder";

function lastSeparatorIndex(path: string): number {
  return Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
}

function isDriveRoot(parentWithSeparator: string): boolean {
  return /^(?:[\\/]{2}\?[\\/])?[A-Za-z]:[\\/]+$/.test(parentWithSeparator);
}

function isUncShareRoot(path: string): boolean {
  if (!/^[\\/]{2}/.test(path)) return false;
  const parts = path.split(/[\\/]+/).filter(Boolean);
  if (parts.length === 2) return true;
  return parts.length === 4 && parts[0] === "?" && parts[1]?.toUpperCase() === "UNC";
}

function parentLabel(key: string): string {
  if (key.length === 0) return CURRENT_FOLDER_LABEL;
  const parts = key.split(/[\\/]+/).filter(Boolean);
  return parts.at(-1) ?? key;
}

/**
 * Extract a file's parent without Node's host-dependent `path` module.
 *
 * No separator, case, drive, or UNC normalization is performed. `key` is an
 * exact prefix of `input`; drive-root identity includes its original separator.
 */
export function extractParentPath(input: string): ParentPath {
  const separatorIndex = lastSeparatorIndex(input);
  if (separatorIndex < 0) {
    const drive = input.match(/^[A-Za-z]:/u)?.[0];
    if (drive !== undefined) {
      return { key: drive, kind: "drive-relative", label: drive };
    }
    return { key: "", kind: "current", label: CURRENT_FOLDER_LABEL };
  }

  const separator = input[separatorIndex];
  const rawPrefix = input.slice(0, separatorIndex);
  const parentWithSeparator = input.slice(0, separatorIndex + 1);
  if (rawPrefix.length === 0) {
    return { key: separator ?? "", kind: "root", label: separator ?? "" };
  }
  if (/^[\\/]+$/.test(rawPrefix) || isDriveRoot(parentWithSeparator)) {
    return { key: parentWithSeparator, kind: "root", label: parentWithSeparator };
  }

  return {
    key: rawPrefix,
    kind: isUncShareRoot(rawPrefix) ? "root" : "path",
    label: parentLabel(rawPrefix),
  };
}
