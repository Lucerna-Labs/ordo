use std::path::PathBuf;

use ordo_operator_sim::{run_operator_sim, ReportVerdict, SimConfig, SimError};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args(std::env::args().skip(1))?;
    let report = run_operator_sim(config.clone()).await?;
    let json_path = report.write_json(&config.output_dir)?;
    let markdown_path = report.write_markdown(&config.output_dir)?;

    println!("Ordo operator simulator finished: {:?}", report.verdict);
    println!("JSON report: {}", json_path.display());
    println!("Markdown report: {}", markdown_path.display());

    if report.verdict == ReportVerdict::Failed {
        std::process::exit(1);
    }
    Ok(())
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<SimConfig, SimError> {
    let mut config = SimConfig::default();
    let mut iter = args.peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--origin" => {
                config.origin = take_value(&mut iter, "--origin")?;
            }
            "--out" => {
                config.output_dir = PathBuf::from(take_value(&mut iter, "--out")?);
            }
            "--profile" => {
                config.profile = take_value(&mut iter, "--profile")?;
            }
            "--voice" => {
                config.include_voice = true;
            }
            "--strict" => {
                config.strict = true;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                return Err(SimError::Args(format!("unknown argument: {other}")));
            }
        }
    }
    Ok(config)
}

fn take_value(
    args: &mut std::iter::Peekable<impl Iterator<Item = String>>,
    flag: &str,
) -> Result<String, SimError> {
    args.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| SimError::Args(format!("{flag} requires a value")))
}

fn print_help() {
    println!(
        "Ordo operator simulator\n\n\
         Usage:\n  \
         cargo run -p ordo-operator-sim -- [options]\n\n\
         Options:\n  \
         --origin <url>     Control API origin (default: http://127.0.0.1:4141)\n  \
         --out <dir>        Report output directory (default: target/operator-sim)\n  \
         --profile <name>   Scenario profile label (default: baseline)\n  \
         --voice            Include cloud speech endpoint check\n  \
         --strict           Treat warnings as a failed report verdict\n  \
         --help             Show this help"
    );
}
