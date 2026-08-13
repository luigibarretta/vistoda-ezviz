from __future__ import annotations

import json
from base64 import b64encode
from pathlib import Path

import pytest

from ezviz_vtm_bridge.auth import ApiAuthenticator
from ezviz_vtm_bridge.config import BridgeConfig, CameraConfig, read_secret_file
from ezviz_vtm_bridge.errors import ConfigurationError


def test_secret_file_requires_mode_0600(tmp_path: Path) -> None:
    secret = tmp_path / "secret"
    secret.write_text("sufficient-secret", encoding="utf-8")
    secret.chmod(0o644)
    with pytest.raises(ConfigurationError, match="permissions"):
        read_secret_file(secret)
    secret.chmod(0o600)
    assert read_secret_file(secret) == "sufficient-secret"


def test_secret_file_refuses_symlink(tmp_path: Path) -> None:
    target = tmp_path / "target"
    target.write_text("secret", encoding="utf-8")
    target.chmod(0o600)
    link = tmp_path / "link"
    link.symlink_to(target)
    with pytest.raises(ConfigurationError, match="regular file"):
        read_secret_file(link)


def test_secret_file_refuses_missing_and_short(tmp_path: Path) -> None:
    with pytest.raises(ConfigurationError, match="unavailable"):
        read_secret_file(tmp_path / "missing")
    secret = tmp_path / "short"
    secret.write_text("x", encoding="utf-8")
    secret.chmod(0o600)
    with pytest.raises(ConfigurationError, match="short"):
        read_secret_file(secret, minimum_length=2)


@pytest.mark.parametrize("token", ["short", "change-me", "password"])
def test_authenticator_refuses_unsafe_tokens(token: str) -> None:
    with pytest.raises(ConfigurationError):
        ApiAuthenticator(token)


def test_authenticator_accepts_only_exact_bearer() -> None:
    token = "x" * 40
    auth = ApiAuthenticator(token)
    assert auth.accepts(f"Bearer {token}")
    assert not auth.accepts(token)
    assert not auth.accepts(f"bearer {token}")
    assert not auth.accepts(f"Bearer {token}x")
    assert not auth.accepts(None)
    basic = b64encode(f"homeassistant:{token}".encode()).decode()
    assert auth.accepts(f"Basic {basic}")
    wrong_user = b64encode(f"other:{token}".encode()).decode()
    assert not auth.accepts(f"Basic {wrong_user}")
    assert not auth.accepts("Basic invalid!")


def test_camera_validation_hides_vendor_identity_from_alias() -> None:
    camera = CameraConfig.from_mapping("front-door", {"serial": "ABC", "decrypt_video": True})
    assert camera.alias == "front-door"
    assert camera.serial == "ABC"
    with pytest.raises(ConfigurationError):
        CameraConfig.from_mapping("../../escape", {"serial": "ABC"})


@pytest.mark.parametrize(
    ("value", "message"),
    [
        ([], "object"),
        ({}, "no serial"),
        ({"serial": "ABC", "media_key_file": 5}, "must be a string"),
        ({"serial": "ABC", "decrypt_video": "yes"}, "must be boolean"),
    ],
)
def test_camera_mapping_rejects_invalid_shapes(value: object, message: str) -> None:
    with pytest.raises(ConfigurationError, match=message):
        CameraConfig.from_mapping("front", value)


def test_environment_configuration_is_bounded(tmp_path: Path) -> None:
    cameras = tmp_path / "cameras.json"
    cameras.write_text(json.dumps({"front": {"serial": "ABC"}}), encoding="utf-8")
    config = BridgeConfig.from_env(
        {
            "EZVIZ_BRIDGE_CAMERAS_FILE": str(cameras),
            "EZVIZ_BRIDGE_MAX_SUBSCRIBERS": "3",
            "EZVIZ_BRIDGE_MAX_RECORDING_SECONDS": "60",
        }
    )
    assert config.max_subscribers == 3
    assert config.cameras["front"].serial == "ABC"
    with pytest.raises(ConfigurationError, match="between"):
        BridgeConfig.from_env(
            {
                "EZVIZ_BRIDGE_CAMERAS_FILE": str(cameras),
                "EZVIZ_BRIDGE_MAX_SUBSCRIBERS": "10000",
            }
        )


def test_environment_configuration_rejects_invalid_json_and_numbers(tmp_path: Path) -> None:
    cameras = tmp_path / "cameras.json"
    cameras.write_text("not-json", encoding="utf-8")
    with pytest.raises(ConfigurationError, match="invalid"):
        BridgeConfig.from_env({"EZVIZ_BRIDGE_CAMERAS_FILE": str(cameras)})
    cameras.write_text(json.dumps({"front": {"serial": "ABC"}}), encoding="utf-8")
    with pytest.raises(ConfigurationError, match="integer"):
        BridgeConfig.from_env(
            {"EZVIZ_BRIDGE_CAMERAS_FILE": str(cameras), "EZVIZ_BRIDGE_BIND_PORT": "bad"}
        )
    with pytest.raises(ConfigurationError, match="numeric"):
        BridgeConfig.from_env(
            {"EZVIZ_BRIDGE_CAMERAS_FILE": str(cameras), "EZVIZ_BRIDGE_IDLE_GRACE": "bad"}
        )
    cameras.write_text("{}", encoding="utf-8")
    with pytest.raises(ConfigurationError, match="at least one"):
        BridgeConfig.from_env({"EZVIZ_BRIDGE_CAMERAS_FILE": str(cameras)})
