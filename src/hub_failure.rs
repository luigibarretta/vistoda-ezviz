use crate::error::BridgeError;

#[derive(Clone, Copy)]
pub enum StreamFailure {
    CameraOffline,
    UpstreamUnavailable,
}

impl StreamFailure {
    pub const fn from_error(error: &BridgeError) -> Self {
        if matches!(error, BridgeError::CameraOffline) {
            Self::CameraOffline
        } else {
            Self::UpstreamUnavailable
        }
    }

    pub const fn into_error(self) -> BridgeError {
        match self {
            Self::CameraOffline => BridgeError::CameraOffline,
            Self::UpstreamUnavailable => BridgeError::UpstreamUnavailable,
        }
    }
}
