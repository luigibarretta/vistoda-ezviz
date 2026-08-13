from __future__ import annotations

import sys
from unittest.mock import Mock

from ezviz_vtm_bridge import cli


def test_cli_composes_config_transport_and_app(monkeypatch) -> None:  # type: ignore[no-untyped-def]
    config = Mock(
        bind_host="127.0.0.1",
        bind_port=9999,
        ezviz_token_file="token-path",  # noqa: S106 - path, not credential
        upstream_timeout_seconds=12,
    )
    transport = Mock()
    app = Mock()
    monkeypatch.setattr(sys, "argv", ["ezviz-vtm-bridge", "serve"])
    monkeypatch.setattr(cli.BridgeConfig, "from_env", Mock(return_value=config))
    token_loader = Mock(return_value=transport)
    monkeypatch.setattr(cli.PyezvizTransport, "from_token_file", token_loader)
    monkeypatch.setattr(cli, "create_app", Mock(return_value=app))
    runner = Mock()
    monkeypatch.setattr(cli.web, "run_app", runner)
    cli.main()
    token_loader.assert_called_once_with("token-path", 12)
    runner.assert_called_once_with(app, host="127.0.0.1", port=9999)
