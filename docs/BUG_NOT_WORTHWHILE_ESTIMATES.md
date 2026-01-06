# Bug: NOT_WORTHWHILE Files Show Estimates in Analysis Tab

## Symptom
Files with `status: "not_worthwhile"` in `conversion_history.json` display size/time estimates in the Analysis tab instead of "Skip". Queue popup correctly identifies them as "not worth converting".

## Confirmed Scenario
1. File marked NOT_WORTHWHILE in previous session
2. App restarted, Analysis tab opened
3. "Basic Scan" clicked
4. Savings column shows `~XX MB` instead of `Skip`
5. Add-to-queue popup correctly says "not worth converting"

## Diagnostic Testing Results

A diagnostic script (`tools/debug_path_hash.py`) was created to test the suspected code paths. Results for a known NOT_WORTHWHILE file:

### ✓ Path Hash Computation - WORKS
All path separator variations produce identical hashes:
```
Variation 1 (mixed separators):  hash=f7fe80c910d00cca ✓
Variation 2 (all backslash):     hash=f7fe80c910d00cca ✓
Variation 3 (all forward slash): hash=f7fe80c910d00cca ✓
```
**Evidence**: `normalize_path()` correctly normalizes all variations to the same canonical form.

### ✓ Index Lookup - WORKS
```
index.get('f7fe80c910d00cca'):
  Found! status=FileStatus.NOT_WORTHWHILE

index.lookup_file(path):
  Found! status=FileStatus.NOT_WORTHWHILE, hash=f7fe80c910d00cca
```
**Evidence**: Both `index.get()` and `index.lookup_file()` find the correct record.

### ✓ Cache Validation - PASSES
```
STORED in history:
  file_size_bytes: 47665732
  file_mtime: 1558293495.0

ACTUAL on disk:
  file size: 47665732
  file mtime: 1558293495.0

Size matches: True
Mtime matches: True (tolerance: 1.0 sec)
Mtime difference: 0.00 seconds
✓ Cache would be HIT - no ffprobe needed
```
**Evidence**: Size and mtime match exactly, so `analyze_one_file()` would return early (cache hit) without calling `_analyze_file()`.

### ✓ Display Function - WORKS
```
Input record status: FileStatus.NOT_WORTHWHILE
Input record status type: <enum 'FileStatus'>
Are they equal? True

Display output:
  savings_str: 'Skip'  <-- Correct!
  tag: 'skip'          <-- Correct!
```
**Evidence**: `compute_analysis_display_values()` correctly returns "Skip" for NOT_WORTHWHILE records.

### ✓ Analysis Tab Simulation - WORKS
```
Simulating os.path.join(folder, filename) -> normcase()
compute_path_hash() -> f7fe80c910d00cca
Matches stored hash? True
```
**Evidence**: The path as constructed by the Analysis tab produces the same hash as stored.

## Hypotheses DISPROVEN

### ~~Path hash mismatch~~ - DISPROVEN
Original theory: Path hash computed at lookup time differs from stored hash.
**Disproven by**: All path variations produce identical hash `f7fe80c910d00cca`.

### ~~Enum vs string comparison failure~~ - DISPROVEN
Original theory: `record.status == FileStatus.NOT_WORTHWHILE` fails due to type mismatch.
**Disproven by**: Diagnostic shows `type(record.status) = <enum 'FileStatus'>` and comparison returns `True`.

### ~~Cache miss triggering _analyze_file()~~ - DISPROVEN
Original theory: Stale mtime causes cache miss, then `_analyze_file()` creates new SCANNED record.
**Disproven by**: Mtime difference is 0.00 seconds - cache would be HIT, `_analyze_file()` never called.

### ~~index.get() returning None~~ - DISPROVEN
Original theory: `index.get(path_hash)` returns None despite record existing.
**Disproven by**: `index.get('f7fe80c910d00cca')` successfully finds the record.

## Key Insight: Cache Hit vs Cache Miss

The bug behavior depends on whether the file's cache validation passes:

### Cache Validation Check
Both `incremental_scan_thread` (initial scan) and `analyze_one_file` (Basic Scan) perform:
```python
record = index.lookup_file(file_path)
if record and record.file_size_bytes == file_size and mtimes_match(record.file_mtime, file_mtime):
    # Cache HIT - use cached values
else:
    # Cache MISS - show defaults / run ffprobe
```

### Two Different Code Paths

**Cache HIT (size + mtime match):**
- Initial scan: Shows `compute_analysis_display_values(record)` → "Skip" ✓
- Basic Scan: `analyze_one_file()` returns early, `batch_update_tree_rows()` re-fetches → "Skip" ✓
- **Bug should NOT manifest**

**Cache MISS (size or mtime changed):**
- Initial scan: Shows "—" (defaults)
- Basic Scan: Calls `_analyze_file()` which runs ffprobe
- `_analyze_file()` SHOULD preserve NOT_WORTHWHILE status at lines 254-261
- If it doesn't, a new SCANNED record is created → estimates shown
- **Bug WOULD manifest here**

### The Tested File Has Cache HIT
The diagnostic showed mtime difference of 0.00 seconds - this file has a cache HIT and would NOT exhibit the bug. We need to find files with cache MISSES to reproduce the bug.

## Remaining Investigation Areas

1. **Find files with cache misses**
   - The diagnostic now scans all NOT_WORTHWHILE records
   - Identifies files where mtime/size has changed since marked NOT_WORTHWHILE
   - These files would trigger `_analyze_file()` code path

2. **Investigate `_analyze_file()` status preservation**
   - Lines 254-261 check: `if cached and cached.status in (NOT_WORTHWHILE, ...)`
   - If `cached` is None or status check fails, new SCANNED record is created
   - Need to verify this logic works for cache-miss files

3. **Why would mtime change?**
   - File accessed/touched after being marked NOT_WORTHWHILE
   - Backup software modifying timestamps
   - File copied/moved

## Next Steps
1. **Run updated diagnostic** to identify NOT_WORTHWHILE files with cache misses
2. **If cache misses found**: Test one of those files specifically
3. **If no cache misses**: Bug may be in a different code path than suspected

## Key Files
| File | Lines | Purpose |
|------|-------|---------|
| `src/folder_analysis.py` | 246-261 | Cache lookup + status preservation |
| `src/gui/tree_display.py` | 164-167 | Status → "Skip" display mapping |
| `src/gui/analysis_scanner.py` | 96-107 | Initial scan cache check |
| `src/gui/analysis_scanner.py` | 236-251 | Basic Scan cache check |
| `src/gui/analysis_tree.py` | 57-108 | `batch_update_tree_rows()` implementation |

## Diagnostic Tool
Run `python tools/debug_path_hash.py` on Windows to test path hashing and lookup for a specific file. Edit the `stored_hash` and `stored_path` variables in the script to test different files.
