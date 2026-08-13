use std::{
    io::{self, Write},
    path::PathBuf,
    sync::Arc,
};

use clap::{Parser, Subcommand};

use crate::{
    api::{Runtime, router},
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
    let transport = Arc::new(
        EzvizTransport::from_token_file(
            config.ezviz_token_file.clone(),
            config.upstream_timeout_seconds,
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
