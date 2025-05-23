use clap::{Parser, Subcommand};
use git_tracker_rs::cli::messages::ControlCommand;
use git_tracker_rs::config::PartialRepoConfig;
use std::io::Write;
use std::net::TcpStream;

#[derive(Parser)]
#[command(name = "repo-tracker")]
#[command(about = "Control CLI for the repo tracker", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Check {
        name: String,
    },
    Shutdown {
        name: String,
    },
    ShutdownAll,
    UpdateConfig {
        #[arg(short, long)]
        name: String,

        #[arg(long)]
        interval: Option<humantime::Duration>,

        #[arg(long)]
        github_token: Option<String>,

        #[arg(long)]
        clear_token: bool,

        #[arg(long)]
        url: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cmd = match cli.command {
        Commands::Check { name } => ControlCommand::CheckNow(name),
        Commands::Shutdown { name } => ControlCommand::Shutdown(name),
        Commands::ShutdownAll => ControlCommand::ShutdownAll,
        Commands::UpdateConfig {
            name,
            interval,
            github_token,
            clear_token,
            url,
        } => {
            let update = PartialRepoConfig {
                name,
                interval: interval.map(|d| d.into()),
                github_token: if clear_token {
                    Some(None)
                } else {
                    github_token.map(Some)
                },
                url,
            };

            ControlCommand::UpdateConfigPartial(update)
        }
    };

    let mut stream = TcpStream::connect("127.0.0.1:4000")?;
    let msg = serde_json::to_vec(&cmd)?;

    stream.write_all(&msg)?;
    println!("Message sent");

    Ok(())
}
