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
    /// Import the microSD clips of a camera-local day (YYYY-MM-DD) not yet
    /// imported into the destination; repeat for several days. Each clip
    /// wakes the camera.
    #[arg(long, conflicts_with = "sd_start")]
    sd_day: Vec<String>,
    /// Clip attempts per day and run; 0 means no limit.
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
    if !args.sd_day.is_empty() {
        let mut results = Vec::new();
        for day in args.sd_day {
            let limits = SdDay {
                day,
                max_clips: match args.sd_max_clips {
                    0 => usize::MAX,
                    limit => limit,
                },
                max_seconds: args.sd_max_seconds,
            };
            match pull_day(&request, &limits).await {
                Ok(pulled) => results.extend(pulled),
                Err(error) => fail(&error),
            }
        }
        print_json(&results);
        return;
    }
    match pull(request).await {
        Ok(result) => print_json(&result),
        Err(error) => fail(&error),
    }
}

/// Bridge errors carry no token, URL query or serial; journald keeps them.
fn fail(error: &ezviz_vtm_bridge::error::BridgeError) -> ! {
    eprintln!("scenetrove-pull: {error}");
    std::process::exit(1)
}

fn print_json<T: serde::Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => println!("{json}"),
        Err(_) => std::process::exit(1),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Arguments;

    const BASE: [&str; 7] = [
        "scenetrove-pull",
        "--base-url",
        "http://bridge:8765",
        "--camera",
        "front",
        "--token-file",
        "/t",
    ];

    fn parse(extra: &[&str]) -> Result<Arguments, clap::Error> {
        Arguments::try_parse_from(BASE.iter().chain(extra).chain(&["--destination", "/o"]))
    }

    #[test]
    fn several_days_and_an_unlimited_clip_budget_parse() {
        let args = parse(&[
            "--sd-day",
            "2026-10-23",
            "--sd-day",
            "2026-10-24",
            "--sd-max-clips",
            "0",
        ])
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(args.sd_day, ["2026-10-23", "2026-10-24"]);
        assert_eq!(args.sd_max_clips, 0);
    }

    #[test]
    fn a_single_recording_still_needs_its_idempotency_key() {
        assert!(parse(&["--duration", "15"]).is_err());
        assert!(parse(&["--duration", "15", "--idempotency-key", "key-0001"]).is_ok());
        assert!(parse(&["--sd-day", "2026-10-24", "--idempotency-key", "key-0001"]).is_err());
        assert!(parse(&["--sd-day", "2026-10-24", "--duration", "15"]).is_err());
    }
}
