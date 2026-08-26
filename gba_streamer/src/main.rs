use anyhow::Result;
use clap::{Parser, Subcommand};
use gba_streamer::udp_host::run_udp_host;
use gba_streamer::web_host::run_web_host;

#[derive(Parser)]
#[command(name = "gba_streamer", about = "⚡ Ultra Low Latency Bit-Exact GBA Streamer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Host {
        #[arg(long, default_value = "/usr/lib64/libretro/mgba_libretro.so")]
        core: String,
        #[arg(long, default_value = "/tmp/test_rom.gba")]
        rom: String,
        #[arg(long, default_value = "0.0.0.0:48500")]
        bind: String,
        #[arg(long)]
        client: Option<String>,
        #[arg(long, default_value_t = 0)]
        frames: u64,
    },
    WebHost {
        #[arg(long, default_value = "/usr/lib64/libretro/mgba_libretro.so")]
        core: String,
        #[arg(long, default_value = "/tmp/test_rom.gba")]
        rom: String,
        #[arg(long, default_value = "0.0.0.0:48500")]
        bind: String,
        #[arg(long, default_value_t = 0)]
        frames: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Host { core, rom, bind, client, frames } => {
            run_udp_host(core, rom, bind, client, frames).await?;
        }
        Commands::WebHost { core, rom, bind, frames: _ } => {
            run_web_host(core, rom, bind).await?;
        }
    }

    Ok(())
}
