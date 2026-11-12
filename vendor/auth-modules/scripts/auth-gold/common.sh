#!/usr/bin/env bash

existing_roots=()
app_roots=()
common_globs=()
code_globs=()
runtime_globs=()
status=0

resolve_studioflow_root() {
  if [[ -n "${AUTH_GOLD_STUDIOFLOW_ROOT:-}" ]]; then
    printf '%s\n' "$AUTH_GOLD_STUDIOFLOW_ROOT"
  elif [[ -d "/home/ansible/repos/studioflow/wt-develop" ]]; then
    printf '%s\n' "/home/ansible/repos/studioflow/wt-develop"
  else
    printf '%s\n' "/opt/studioflow"
  fi
}

studioflow_root="$(resolve_studioflow_root)"
auth_modules_root="$(cd "${script_dir}/.." && pwd -P)"

init_auth_gold() {
  if ! command -v rg >/dev/null 2>&1; then
    echo "error: ripgrep (rg) is required" >&2
    exit 2
  fi

  local roots=()
  if (($# > 0)); then
    roots=("$@")
  else
    roots=(
      "/home/ansible/repos/blackstart"
      "/home/ansible/repos/coveyos"
      "/home/ansible/repos/klaxond"
      "/home/ansible/repos/scenetrove"
      "/home/ansible/repos/quadro-rust-resume"
      "$studioflow_root"
      "$auth_modules_root"
    )
  fi

  local root
  for root in "${roots[@]}"; do
    if [[ -d "$root" ]]; then
      existing_roots+=("$root")
      if [[ "$(cd "$root" && pwd -P)" != "$auth_modules_root" ]]; then
        app_roots+=("$root")
      fi
    else
      echo "warn: skipping missing root: $root" >&2
    fi
  done

  if ((${#existing_roots[@]} == 0)); then
    echo "error: no repository roots to scan" >&2
    exit 2
  fi

  common_globs=(
    "--hidden"
    "-g" "!.git/**"
    "-g" "!**/.git/**"
    "-g" "!target/**"
    "-g" "!node_modules/**"
    "-g" "!dist/**"
    "-g" "!build/**"
    "-g" "!vendor/**"
    "-g" "!**/vendor/**"
    "-g" "!static/vendor/**"
    "-g" "!frontend/dist/**"
    "-g" "!coverage/**"
    "-g" "!__pycache__/**"
    "-g" "!**/scripts/auth-gold/**"
    "-g" "!**/scripts/check-auth-gold-standard.sh"
    "-g" "!BACKLOG_HISTORIC.md"
    "-g" "!CHANGELOG.md"
    "-g" "!docs/**"
  )

  code_globs=(
    "-g" "*.rs"
    "-g" "*.toml"
    "-g" "*.lock"
    "-g" "*.ts"
    "-g" "*.tsx"
    "-g" "*.js"
    "-g" "*.jsx"
    "-g" "*.mjs"
    "-g" "*.svelte"
    "-g" "*.html"
    "-g" "*.yaml"
    "-g" "*.yml"
    "-g" "*.json"
    "-g" "*.sh"
    "-g" "*.py"
    "-g" "Makefile"
    "-g" "Dockerfile"
  )

  runtime_globs=(
    "${common_globs[@]}"
    "-g" "!**/__tests__/**"
    "-g" "!**/tests/**"
    "-g" "!**/tests.rs"
    "-g" "!**/*.test.*"
    "-g" "!**/*.spec.*"
    "-g" "!**/e2e/**"
  )
}

check_absent() {
  local title=$1
  local pattern=$2
  shift 2

  local output
  set +e
  output=$(rg -n -S "$pattern" "$@" "${existing_roots[@]}" 2>/dev/null)
  local rc=$?
  set -e

  if [[ $rc -eq 0 ]]; then
    echo
    echo "FAIL: $title"
    echo "$output"
    status=1
  elif [[ $rc -gt 1 ]]; then
    echo
    echo "FAIL: $title scan errored (rg exit $rc)"
    status=1
  else
    echo "ok: $title"
  fi
}

check_present_per_repo() {
  local title=$1
  local pattern=$2
  shift 2
  local root
  for root in "$@"; do
    [[ -d "$root" ]] || continue
    set +e
    rg -q -S "$pattern" "${common_globs[@]}" "$root" 2>/dev/null
    local rc=$?
    set -e
    if [[ $rc -eq 0 ]]; then
      echo "ok: $title in $root"
    elif [[ $rc -eq 1 ]]; then
      echo
      echo "FAIL: $title missing in $root"
      status=1
    else
      echo
      echo "FAIL: $title scan errored in $root (rg exit $rc)"
      status=1
    fi
  done
}

check_executable_file_per_repo() {
  local title=$1
  local relative_path=$2
  shift 2
  local root
  for root in "$@"; do
    [[ -d "$root" ]] || continue
    if [[ -x "$root/$relative_path" ]]; then
      echo "ok: $title in $root"
    else
      echo
      echo "FAIL: $title missing or not executable in $root"
      echo "$root/$relative_path"
      status=1
    fi
  done
}

finish_auth_gold() {
  if [[ $status -eq 0 ]]; then
    echo
    echo "auth gold standard guardrails passed"
  else
    echo
    echo "auth gold standard guardrails failed"
  fi
  exit "$status"
}
