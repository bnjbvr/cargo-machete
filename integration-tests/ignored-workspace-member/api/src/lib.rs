use clap::Parser;

#[derive(Parser)]
pub struct Config {
    #[clap(long)]
    pub port: u16,
}

pub fn create_config() -> Config {
    Config::parse()
}