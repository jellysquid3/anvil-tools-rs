use clap::Parser;

mod region;
mod commands;

fn main() {
    let opts: Opts = Opts::parse();

    match opts.command {
        Command::Pack(v) => commands::archive::pack_files(&v)
            .expect("Failed to pack files"),
        Command::Unpack(v) => commands::archive::unpack_files(&v)
            .expect("Failed to strip files"),
        Command::Strip(v) => commands::strip::strip_files(&v)
            .expect("Failed to strip files")
    }
}

#[derive(Parser)]
#[clap()]
struct Opts {
    #[clap(subcommand)]
    command: Command
}

#[derive(Parser)]
enum Command {
    Strip(commands::strip::Options),
    Pack(commands::archive::PackOptions),
    Unpack(commands::archive::UnpackOptions)
}