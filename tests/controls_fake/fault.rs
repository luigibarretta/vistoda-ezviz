//! Failures the synthetic control source can inject into its reads.

use std::time::Duration;

use ezviz_vtm_bridge::error::BridgeError;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fault {
    #[default]
    None,
    /// EZVIZ answered with an error (as in the 2026-10-20 HTTP 500 burst).
    Upstream,
    /// EZVIZ refused the session and its refresh.
    Reauth,
    /// EZVIZ never answers within the read deadline.
    Hang,
}

impl Fault {
    pub async fn inject(self) -> Result<(), BridgeError> {
        match self {
            Self::None => Ok(()),
            Self::Upstream => Err(BridgeError::Upstream("EZVIZ API returned HTTP 500".into())),
            Self::Reauth => Err(BridgeError::ReauthRequired),
            Self::Hang => {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                Err(BridgeError::Upstream("too late".into()))
            }
        }
    }
}
