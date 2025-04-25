use std::net::SocketAddr;

use anyhow::Result;
use clap::Parser;
use tokio::net::UdpSocket;

pub mod server;

#[derive(Parser)]
struct Args {
    #[clap(short, long, default_value = "127.0.0.1:5050")]
    addr: SocketAddr,
}

async fn server(host_addr: &SocketAddr) -> Result<()> {
    let sock = UdpSocket::bind(host_addr).await?;

    let mut server = server::Server::new();

    let mut buf = vec![0u8; rosc::decoder::MTU];

    'recv: loop {
        match sock.recv_from(&mut buf).await {
            Ok((size, client_addr)) => {
                let packet = match rosc::decoder::decode_udp(&buf[..size]) {
                    Ok((_, packet)) => packet,
                    Err(e) => {
                        log::error!("Malformed packet: {e}");
                        continue 'recv;
                    }
                };

                log::debug!("[{}] {:?}", client_addr, &packet);

                let resps = server.apply_osc(&packet)?;
                for resp in resps {
                    let buf = rosc::encoder::encode(&resp.to_osc())?;
                    sock.send_to(&buf, client_addr).await?;
                }
            }
            Err(e) => {
                log::error!("recv_from failed: {}", e);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    server(&args.addr).await?;
    Ok(())
}
