#!/usr/bin/env bash

tracked_apps=(
  "/home/ansible/repos/blackstart"
  "/home/ansible/repos/coveyos"
  "/home/ansible/repos/klaxond"
  "/home/ansible/repos/scenetrove"
  "/home/ansible/repos/quadro-rust-resume"
  "$studioflow_root"
)

run_presence_checks() {
  check_shared_contracts
  check_runtime_method_contracts
}

check_shared_contracts() {
  check_present_per_repo "canonical password policy endpoint" '/api/auth/password-policy' "${tracked_apps[@]}"

  check_present_per_repo "shared password module" 'auth_modules::password' "${tracked_apps[@]}"

  check_present_per_repo "canonical auth methods endpoint" '/api/auth/methods' "${tracked_apps[@]}"

  check_present_per_repo "shared auth method vocabulary" 'auth_modules::methods' "${tracked_apps[@]}"

  check_present_per_repo "shared auth brute-force policy" \
    'GOLD_AUTH_|gold_auth_account_failure_policy' "${tracked_apps[@]}"

  check_present_per_repo "shared OIDC protocol client" \
    'auth_modules::oidc|oidc-(async|blocking)' "${tracked_apps[@]}"

  check_executable_file_per_repo "repo-local auth gold standard wrapper" \
    "scripts/check-auth-gold-standard.sh" "${tracked_apps[@]}"

  check_present_per_repo "shared OIDC PKCE primitives" \
    'auth_modules::oidc_pkce' \
    "/home/ansible/repos/coveyos" \
    "/home/ansible/repos/klaxond" \
    "/home/ansible/repos/scenetrove" \
    "/home/ansible/repos/quadro-rust-resume"

  check_present_per_repo "shared TOTP module where not intentionally using a maintained TOTP library" \
    'auth_modules::totp' \
    "/home/ansible/repos/klaxond" \
    "/home/ansible/repos/scenetrove" \
    "/home/ansible/repos/quadro-rust-resume" \
    "$studioflow_root"

  check_present_per_repo "shared LDAP adapter where runtime LDAP has been normalized" \
    'auth_modules::ldap' \
    "/home/ansible/repos/coveyos" \
    "/home/ansible/repos/scenetrove" \
    "/home/ansible/repos/klaxond" \
    "/home/ansible/repos/quadro-rust-resume" \
    "$studioflow_root"
}

check_runtime_method_contracts() {
  check_present_per_repo "SceneTrove magic-link auth runtime method" \
    '/api/auth/magic/request|magic_link_enabled|MAGIC_LINK => config\.magic_link_enabled' \
    "/home/ansible/repos/scenetrove"

  check_present_per_repo "SceneTrove hardware-key method mapped to WebAuthn" \
    'HARDWARE_KEY => config\.passkeys_enabled' \
    "/home/ansible/repos/scenetrove"

  check_present_per_repo "SceneTrove LDAP auth runtime method" \
    'LDAP => config\.ldap_login_enabled|ldap_identity_to_user|auth_modules::ldap' \
    "/home/ansible/repos/scenetrove"

  check_present_per_repo "Klaxond magic-link auth runtime method" \
    '/api/auth/magic/request|magic_link_enabled|MAGIC_LINK => auth::magic_link_enabled' \
    "/home/ansible/repos/klaxond"

  check_present_per_repo "Klaxond hardware-key method mapped to WebAuthn" \
    'HARDWARE_KEY => auth_cfg\.webauthn\.enabled' \
    "/home/ansible/repos/klaxond"

  check_present_per_repo "Klaxond LDAP auth runtime method" \
    'LDAP => auth::ldap_login_enabled|mode == "ldap"|auth_modules::ldap' \
    "/home/ansible/repos/klaxond"

  check_present_per_repo "CoveyOS magic-link auth runtime method" \
    '/api/auth/magic/request|magic_request|MAGIC_LINK => local && configured\.password' \
    "/home/ansible/repos/coveyos"

  check_present_per_repo "CoveyOS hardware-key method mapped to configured WebAuthn" \
    'HARDWARE_KEY => local && configured\.hardware_key' \
    "/home/ansible/repos/coveyos"

  check_present_per_repo "CoveyOS LDAP auth runtime method" \
    'AuthMode::Ldap|LDAP => ldap|auth_modules::ldap' \
    "/home/ansible/repos/coveyos"

  check_present_per_repo "StudioFlow magic-link auth runtime method" \
    '/api/auth/magic/request|magic_link_request|MAGIC_LINK => true' \
    "$studioflow_root"

  check_present_per_repo "StudioFlow hardware-key method mapped to WebAuthn" \
    'HARDWARE_KEY => webauthn_origin_and_rp_id\(&state\)\.is_ok\(\)' \
    "$studioflow_root"

  check_present_per_repo "Quadro LDAP auth runtime method" \
    'auth_ldap::ldap_login_enabled|auth_modules::ldap|auth\.ldap' \
    "/home/ansible/repos/quadro-rust-resume"

  check_present_per_repo "Quadro magic-link auth runtime method" \
    '/api/auth/magic/request|auth_magic_link|magic_link' \
    "/home/ansible/repos/quadro-rust-resume"

  check_present_per_repo "Quadro hardware-key method mapped to WebAuthn" \
    'HARDWARE_KEY => method_enabled\(db, "passkey"\)\.await' \
    "/home/ansible/repos/quadro-rust-resume"
}
