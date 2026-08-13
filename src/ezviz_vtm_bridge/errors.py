"""Domain errors whose messages are safe to expose."""


class BridgeError(Exception):
    """Base bridge error."""


class ConfigurationError(BridgeError):
    """Configuration is missing or unsafe."""


class CameraNotFoundError(BridgeError):
    """A configured camera alias was not found."""


class StreamStoppedError(BridgeError):
    """The bridge intentionally stopped an upstream stream."""


class CapacityError(BridgeError):
    """A configured resource bound was reached."""


class RecordingError(BridgeError):
    """A finite recording could not be produced."""
