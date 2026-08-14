#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    if let Err(error) = ezviz_vtm_bridge::cli::run().await {
        tracing::error!(error = %error, error_type = "fatal", "bridge stopped");
        std::process::exit(1);
    }
}
