use std::path::PathBuf;

use clap::Parser;
use ezviz_vtm_bridge::scenetrove::{PullRequest, pull};

#[derive(Parser)]
#[command(name = "scenetrove-pull", version, about)]
struct Arguments {
    #[arg(long)]
    base_url: String,
    #[arg(long)]
    camera: String,
    #[arg(long)]
    duration: u64,
    #[arg(long)]
    idempotency_key: String,
    #[arg(long)]
    token_file: PathBuf,
    #[arg(long)]
    destination: PathBuf,
    #[arg(long, default_value_t = 180)]
    timeout: u64,
    #[arg(long, default_value_t = 256 * 1024 * 1024)]
    max_bytes: u64,
}

#[tokio::main]
async fn main() {
    let args = Arguments::parse();
    let request = PullRequest {
        base_url: args.base_url,
        camera: args.camera,
        duration_seconds: args.duration,
        idempotency_key: args.idempotency_key,
        token_file: args.token_file,
        destination: args.destination,
        timeout_seconds: args.timeout,
        max_bytes: args.max_bytes,
    };
    match pull(request).await {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(json) => println!("{json}"),
            Err(_) => std::process::exit(1),
        },
        Err(_) => std::process::exit(1),
    }
}
