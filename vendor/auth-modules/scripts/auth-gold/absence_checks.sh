#!/usr/bin/env bash

run_absence_checks() {
  check_absent \
    "bcrypt is not present in active code, manifests, or lockfiles" \
    'bcrypt|\$2[aby]\$' \
    "${code_globs[@]}" "${common_globs[@]}"

  check_absent \
    "SHA1 password fallback is not present in active code, manifests, or lockfiles" \
    '(PASSWORD_SHA1|password_sha1|passwordSha1|basic_password_sha1|local_password_sha1|AUTH_[A-Z0-9_]*PASSWORD[A-Z0-9_]*SHA1|SHA1[A-Z0-9_]*PASSWORD|sha1[_-]?password|password[_-]?sha1|verify_sha1_password)' \
    "${code_globs[@]}" "${common_globs[@]}"

  check_absent \
    "runtime code does not call or mount root /auth/* API paths" \
    '(route\([^;]*["`]/auth/|\("(GET|POST|PUT|PATCH|DELETE)",[[:space:]]*["`]/auth/|fetch\(["`]/auth/|http\.(get|post|put|patch|delete)\(["`]/auth/|axios\.(get|post|put|patch|delete)\(["`]/auth/|href=["`]/auth/|location\.(href|assign|replace)\(["`]/auth/)' \
    "${code_globs[@]}" "${runtime_globs[@]}"

  check_no_direct_openidconnect
  check_no_local_oidc_runtime
  check_no_custom_limiter
  check_no_local_bruteforce_policy
}

check_no_direct_openidconnect() {
  if ((${#app_roots[@]} == 0)); then
    echo "ok: app manifests do not depend on openidconnect directly; use auth_modules::oidc"
    return
  fi
  local output rc
  set +e
  output=$(rg -n -S 'openidconnect[[:space:]]*=' \
    "-g" "Cargo.toml" "${common_globs[@]}" "${app_roots[@]}" 2>/dev/null)
  rc=$?
  set -e
  report_absence_scan \
    "$rc" "$output" \
    "app manifests do not depend on openidconnect directly; use auth_modules::oidc" \
    "direct openidconnect manifest"
}

check_no_local_oidc_runtime() {
  if ((${#app_roots[@]} == 0)); then
    echo "ok: app runtime OIDC code does not implement discovery, JWKS, token exchange, or ID-token validation locally"
    return
  fi
  local output rc
  set +e
  output=$(rg -n -S \
    '(CoreProviderMetadata|AuthorizationCode|PkceCode|decode_header|JwkSet|jsonwebtoken::jwk|verify_id_token|jwk_matches_algorithm|/\.well-known/openid-configuration|jwks_uri)' \
    "-g" "*.rs" "${runtime_globs[@]}" "${app_roots[@]}" 2>/dev/null)
  rc=$?
  set -e
  report_absence_scan \
    "$rc" "$output" \
    "app runtime OIDC code does not implement discovery, JWKS, token exchange, or ID-token validation locally" \
    "custom OIDC runtime"
}

check_no_custom_limiter() {
  if ((${#app_roots[@]} == 0)); then
    echo "ok: auth-related Rust code does not define custom in-memory rate buckets outside auth-modules"
    return
  fi
  local output rc
  set +e
  output=$(rg -n -S \
    '(HashMap<[^>]*(Vec<Instant>|VecDeque<Instant>)|VecDeque<Instant>|AUTH_RATE_WINDOW|LOGIN_RATE_WINDOW|PASSKEY_RATE_WINDOW|MAGIC_LINK_RATE_WINDOW)' \
    "-g" "*auth*.rs" "-g" "*passkey*.rs" "-g" "*magic*.rs" "-g" "*pat*.rs" \
    "${runtime_globs[@]}" "${app_roots[@]}" 2>/dev/null)
  rc=$?
  set -e
  report_absence_scan \
    "$rc" "$output" \
    "auth-related Rust code does not define custom in-memory rate buckets outside auth-modules" \
    "auth-related Rust limiter"
}

check_no_local_bruteforce_policy() {
  if ((${#app_roots[@]} == 0)); then
    echo "ok: auth brute-force thresholds are not redeclared in app auth code"
    return
  fi
  local output rc
  set +e
  output=$(rg -n -S \
    '(LOCAL_AUTH_MAX_FAILURES|LOCAL_AUTH_LOCKOUT_WINDOW|AUTH_SECURITY_MAX_FAILURES|AUTH_SECURITY_LOCKOUT_WINDOW|AUTH_FAILURE_MAX|AUTH_FAILURE_WINDOW_SECS|AUTH_LOCKOUT_SECS|PASSKEY_AUTH_FAILURE_MAX|PASSKEY_AUTH_FAILURE_WINDOW|RATE_LIMIT_MAX_ATTEMPTS|RATE_LIMIT_WINDOW|PASSWORD_LOGIN_DEFAULT_MAX_FAILURES|PASSWORD_LOGIN_DEFAULT_LOCKOUT_MINUTES)' \
    "-g" "*auth*.rs" "-g" "*passkey*.rs" "-g" "*magic*.rs" \
    "${runtime_globs[@]}" "${app_roots[@]}" 2>/dev/null)
  rc=$?
  set -e
  report_absence_scan \
    "$rc" "$output" \
    "auth brute-force thresholds are not redeclared in app auth code" \
    "auth brute-force threshold"
}

report_absence_scan() {
  local rc=$1
  local output=$2
  local title=$3
  local error_label=$4

  if [[ $rc -eq 0 ]]; then
    echo
    echo "FAIL: $title"
    echo "$output"
    status=1
  elif [[ $rc -gt 1 ]]; then
    echo
    echo "FAIL: $error_label scan errored (rg exit $rc)"
    status=1
  else
    echo "ok: $title"
  fi
}
