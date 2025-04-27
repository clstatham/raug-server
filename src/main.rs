use std::{net::SocketAddr, str::FromStr};

use anyhow::Result;
use clap::Parser;
use raug::prelude::{AudioBackend, AudioDevice};
use tokio::net::UdpSocket;

#[derive(Parser)]
struct Args {
    #[clap(short, long, default_value = "127.0.0.1:5050")]
    addr: SocketAddr,
    #[clap(short, long, default_value = "0")]
    inputs: usize,
    #[clap(short, long, default_value = "2")]
    outputs: usize,
    #[clap(short, long, value_parser = AudioBackend::from_str, default_value = "default")]
    backend: AudioBackend,
    #[clap(short, long, value_parser = AudioDevice::from_str, default_value = "default")]
    device: AudioDevice,
}

async fn server(args: Args) -> Result<()> {
    let Args {
        addr,
        inputs,
        outputs,
        backend,
        device,
    } = args;
    let sock = UdpSocket::bind(addr).await?;

    let mut server = raug_server::server::Server::new(inputs, outputs, backend, device);

    let mut buf = [0u8; rosc::decoder::MTU];

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
                return Err(e.into());
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    server(args).await?;
    Ok(())
}
