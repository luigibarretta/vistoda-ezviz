# Contributing

Read the Vistoda family [contribution guide](https://github.com/luigibarretta/vistoda-home-assistant/blob/main/CONTRIBUTING.md)
before changing a cross-repository contract.

This repository owns EZVIZ enrollment, camera binding, snapshots and media
transport. The Home Assistant panel belongs in `vistoda-home-assistant`;
app-store metadata belongs in `vistoda-addons`.

Run the validation commands in [README.md](README.md#development-and-quality-gates).
Real-device canaries are opt-in and must leave the upstream and remux pipelines
idle after teardown. Report security issues through [SECURITY.md](SECURITY.md).
