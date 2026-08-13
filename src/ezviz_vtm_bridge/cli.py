"""Command-line entry point."""

from __future__ import annotations

import argparse
import getpass
import logging
from pathlib import Path

from aiohttp import web
from pyezvizapi.exceptions import EzvizAuthVerificationCode

from .app import create_app
from .config import BridgeConfig
from .transport import PyezvizTransport


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(prog="ezviz-vtm-bridge")
    commands = result.add_subparsers(dest="command")
    commands.add_parser("serve")
    enroll = commands.add_parser("enroll")
    enroll.add_argument("--account", required=True)
    enroll.add_argument("--api-region", default="apiieu.ezvizlife.com")
    enroll.add_argument("--token-file", type=Path, required=True)
    enroll.add_argument("--timeout", type=float, default=25)
    return result


def main() -> None:
    arguments = parser().parse_args()
    if arguments.command == "enroll":
        password = getpass.getpass("EZVIZ password (never stored): ")
        try:
            PyezvizTransport.enroll(
                account=arguments.account,
                password=password,
                api_region=arguments.api_region,
                token_path=arguments.token_file,
                timeout_seconds=arguments.timeout,
            )
        except EzvizAuthVerificationCode:
            code = int(getpass.getpass("EZVIZ MFA code: "))
            PyezvizTransport.enroll(
                account=arguments.account,
                password=password,
                api_region=arguments.api_region,
                token_path=arguments.token_file,
                timeout_seconds=arguments.timeout,
                mfa_code=code,
            )
        return
    if arguments.command not in {None, "serve"}:
        raise SystemExit(2)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )
    config = BridgeConfig.from_env()
    transport = PyezvizTransport.from_token_file(
        config.ezviz_token_file, config.upstream_timeout_seconds
    )
    web.run_app(create_app(config, transport), host=config.bind_host, port=config.bind_port)
