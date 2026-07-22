import { describe, expect, it } from "vitest";

import { extractParentPath } from "./parent-path";

describe("extractParentPath", () => {
  it.each([
    ["/file.mkv", "/", "root", "/"],
    ["\\file.mkv", "\\", "root", "\\"],
    ["relative.mkv", "", "current", "Current folder"],
    ["folder/file.mkv", "folder", "path", "folder"],
    ["./file.mkv", ".", "path", "."],
    ["../file.mkv", "..", "path", ".."],
    ["C:\\file.mkv", "C:\\", "root", "C:\\"],
    ["C:/file.mkv", "C:/", "root", "C:/"],
    ["C:file.mkv", "C:", "drive-relative", "C:"],
    ["C:\\one/two\\file.mkv", "C:\\one/two", "path", "two"],
    ["\\\\server\\share\\file.mkv", "\\\\server\\share", "root", "share"],
    ["\\\\server/share\\file.mkv", "\\\\server/share", "root", "share"],
    ["//server/share/file.mkv", "//server/share", "root", "share"],
    ["\\\\?\\C:\\file.mkv", "\\\\?\\C:\\", "root", "\\\\?\\C:\\"],
    ["//?/C:/file.mkv", "//?/C:/", "root", "//?/C:/"],
    ["\\\\?\\UNC\\server\\share\\file.mkv", "\\\\?\\UNC\\server\\share", "root", "share"],
    ["\\\\?\\UNC/server\\share/file.mkv", "\\\\?\\UNC/server\\share", "root", "share"],
    ["//?/UNC/server/share/file.mkv", "//?/UNC/server/share", "root", "share"],
  ] as const)("extracts the exact parent of %s", (input, key, kind, label) => {
    expect(extractParentPath(input)).toEqual({ key, kind, label });
  });

  it("keeps exact spelling and separator identity", () => {
    expect(extractParentPath("C:\\Media\\file.mkv").key).toBe("C:\\Media");
    expect(extractParentPath("c:/media/file.mkv").key).toBe("c:/media");
    expect(extractParentPath("C:\\Media\\file.mkv").key).not.toBe(
      extractParentPath("c:/media/file.mkv").key,
    );
  });

  it("keeps duplicate display basenames as distinct full-path keys", () => {
    const first = extractParentPath("/library/one/Season 1/episode.mkv");
    const second = extractParentPath("/library/two/Season 1/episode.mkv");
    expect(first.label).toBe("Season 1");
    expect(second.label).toBe("Season 1");
    expect(first.key).not.toBe(second.key);
  });
});
