#!/usr/bin/env bash
# WorktreeCreate hook: create Claude Code worktrees under .worktrees/ (shared
# with other agent tools) instead of the default .claude/worktrees/.
# Contract: JSON on stdin (worktree_name, base_ref, cwd); absolute worktree
# path on stdout; non-zero exit aborts creation.
set -euo pipefail
input=$(cat)
name=$(jq -er '.worktree_name' <<<"$input")
base=$(jq -r 'if (.base_ref // "") == "" then "main" else .base_ref end' <<<"$input")
dir=".worktrees/$name"
git worktree add "$dir" -b "$name" "$base" >&2
cd "$dir" && pwd
