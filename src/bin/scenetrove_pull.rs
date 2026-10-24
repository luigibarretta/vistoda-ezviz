use std::path::PathBuf;

use clap::Parser;
use ezviz_vtm_bridge::scenetrove::{
    PullRequest, pull,
    sd::{SdDay, pull_day},
};

#[derive(Parser)]
#[command(name = "scenetrove-pull", version, about)]
struct Arguments {
    #[arg(long)]
    base_url: String,
    #[arg(long)]
    camera: String,
    /// Live capture length in seconds.
    #[arg(
        long,
        required_unless_present_any = ["sd_start", "sd_day"],
        conflicts_with_all = ["sd_start", "sd_day"]
    )]
    duration: Option<u64>,
    /// Import the microSD clips of this camera-local day (YYYY-MM-DD) not yet
    /// imported into the destination; each clip wakes the camera.
    #[arg(long, conflicts_with = "sd_start")]
    sd_day: Option<String>,
    #[arg(long, default_value_t = 10)]
    sd_max_clips: usize,
    #[arg(long, default_value_t = 120)]
    sd_max_seconds: i64,
    /// microSD interval start, epoch seconds (with --sd-end).
    #[arg(long, requires = "sd_end")]
    sd_start: Option<i64>,
    #[arg(long, requires = "sd_start")]
    sd_end: Option<i64>,
    /// Required for one recording; `--sd-day` derives one per clip.
    #[arg(long, required_unless_present = "sd_day", conflicts_with = "sd_day")]
    idempotency_key: Option<String>,
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
        duration_seconds: args.duration.unwrap_or(0),
        sd_interval: args.sd_start.zip(args.sd_end),
        idempotency_key: args.idempotency_key.unwrap_or_default(),
        token_file: args.token_file,
        destination: args.destination,
        timeout_seconds: args.timeout,
        max_bytes: args.max_bytes,
    };
    if let Some(day) = args.sd_day {
        let limits = SdDay {
            day,
            max_clips: args.sd_max_clips,
            max_seconds: args.sd_max_seconds,
        };
        match pull_day(&request, &limits).await {
            Ok(results) => match serde_json::to_string(&results) {
                Ok(json) => println!("{json}"),
                Err(_) => std::process::exit(1),
            },
            Err(_) => std::process::exit(1),
        }
        return;
    }
    match pull(request).await {
        Ok(result) => match serde_json::to_string(&result) {
            Ok(json) => println!("{json}"),
            Err(_) => std::process::exit(1),
        },
        Err(_) => std::process::exit(1),
    }
}
