use std::{
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};

use clap::{Parser, Subcommand};

use crate::{
    api::{Runtime, router},
    canary::{CanaryOptions, run as run_canary},
    config::BridgeConfig,
    error::BridgeError,
    transport::{EzvizTransport, enroll},
};

#[derive(Parser)]
#[command(name = "ezviz-vtm-bridge", version, about)]
pub struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve,
    Healthcheck,
    Canary(CanaryOptions),
    Enroll {
        #[arg(long)]
        account: String,
        #[arg(long, default_value = "eu")]
        api_region: String,
        #[arg(long, default_value = "/data/token.json")]
        token_file: PathBuf,
        #[arg(long, default_value_t = 20)]
        timeout: u64,
    },
}

pub async fn run() -> Result<(), BridgeError> {
    match Arguments::parse().command {
        Command::Serve => serve().await,
        Command::Healthcheck => healthcheck().await,
        Command::Canary(options) => run_canary(options).await.map_err(|error| {
            tracing::error!(detail = %error, "canary failed");
            error
        }),
        Command::Enroll {
            account,
            api_region,
            token_file,
            timeout,
        } => {
            let password = rpassword::prompt_password("Password EZVIZ: ")
                .map_err(|_| BridgeError::Configuration("could not read password".into()))?;
            enroll(
                &account,
                &password,
                &api_region,
                &token_file,
                timeout,
                || {
                    let code = rpassword::prompt_password("Codice MFA EZVIZ: ").map_err(|_| {
                        BridgeError::Configuration("could not read MFA code".into())
                    })?;
                    Ok(code)
                },
            )
            .await?;
            io::stdout().write_all(b"Enrollment completed\n")?;
            Ok(())
        }
    }
}

async fn healthcheck() -> Result<(), BridgeError> {
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?
        .get("http://127.0.0.1:8765/healthz")
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(BridgeError::Upstream("health endpoint is not ready".into()));
    }
    Ok(())
}

async fn serve() -> Result<(), BridgeError> {
    let config = BridgeConfig::from_env()?;
    let address = format!("{}:{}", config.bind_host, config.bind_port);
    if !config.ezviz_token_file.is_file() {
        serve_enrollment(&address, &config).await?;
        if !config.ezviz_token_file.is_file() {
            return Ok(());
        }
    }
    serve_runtime(&address, config).await
}

async fn serve_enrollment(address: &str, config: &BridgeConfig) -> Result<(), BridgeError> {
    let (ready, receiver) = tokio::sync::watch::channel(false);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(bind = %address, version = crate::VERSION, "waiting for EZVIZ enrollment");
    axum::serve(listener, crate::bootstrap::router(config, ready)?)
        .with_graceful_shutdown(enrollment_shutdown(receiver))
        .await?;
    Ok(())
}

async fn serve_runtime(address: &str, config: BridgeConfig) -> Result<(), BridgeError> {
    let transport = Arc::new(
        EzvizTransport::from_token_file(
            config.ezviz_token_file.clone(),
            config.upstream_timeout_seconds,
            config.ffmpeg_path.clone(),
        )
        .await?,
    );
    let runtime = Runtime::build(config, transport)?;
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(bind = %address, version = crate::VERSION, "bridge started");
    axum::serve(listener, router(Arc::clone(&runtime)))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    runtime.close().await;
    Ok(())
}

async fn enrollment_shutdown(mut receiver: tokio::sync::watch::Receiver<bool>) {
    tokio::select! {
        () = shutdown_signal() => {},
        result = receiver.changed() => {
            if result.is_ok() && *receiver.borrow() {
                tracing::info!("EZVIZ enrollment completed; enabling media runtime");
            }
        },
    }
}

async fn shutdown_signal() {
    let interrupt = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        result = interrupt => drop(result),
        () = terminate => {},
    }
}
