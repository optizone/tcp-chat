use chat::server;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = "server", about = "Simple TCP chat room.")]
struct Opt {
    /// Set address of the server
    #[structopt(short, long, default_value = "127.0.0.1:8080")]
    address: String,
}

#[tokio::main]
async fn main() {
    let Opt { address } = Opt::from_args();
    server::run_server(address).await.unwrap();
}
