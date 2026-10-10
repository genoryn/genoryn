use clap::Parser;

#[derive(Parser)]
#[command(about = "Exact GENORYN monetary policy, in gori (10^-12 GNR)")]
struct Args {
    #[arg(long, default_value_t = 1)]
    height: u64,
}

fn main() -> anyhow::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&genoryn_core::emission::supply(Args::parse().height))?
    );
    Ok(())
}
