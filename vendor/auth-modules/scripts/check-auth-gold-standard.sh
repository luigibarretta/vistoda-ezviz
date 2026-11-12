#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

source "$script_dir/auth-gold/common.sh"
source "$script_dir/auth-gold/absence_checks.sh"
source "$script_dir/auth-gold/presence_checks.sh"

init_auth_gold "$@"
run_absence_checks
run_presence_checks
finish_auth_gold
