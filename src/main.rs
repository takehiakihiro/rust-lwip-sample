use anyhow::Result;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use std::net::{IpAddr, SocketAddr};

const MTU: u16 = 1400;

#[repr(C)]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum ArgVerbosity {
    Off = 0,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

#[derive(Parser)]
#[command(author, version, about = "Testing app for tun.", long_about = None)]
struct Args {
    /// echo server address, likes `127.0.0.1:4000`
    #[arg(short, long, value_name = "IP:port", default_value = "127.0.0.1:4000")]
    vpn_addr: SocketAddr,

    /// echo server address, likes `127.0.0.1`
    #[arg(short, long, value_name = "IP address", default_value = "127.0.0.1")]
    server_addr: IpAddr,

    /// tcp timeout
    #[arg(long, value_name = "seconds", default_value = "10")]
    tcp_timeout: u64,

    /// udp timeout
    #[arg(long, value_name = "seconds", default_value = "10")]
    udp_timeout: u64,

    /// Verbosity level
    #[arg(short, long, value_name = "level", value_enum, default_value = "info")]
    pub verbosity: ArgVerbosity,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let default = format!("{:?}", args.verbosity);
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default)).init();

    let mut ipstack_config = IpStackConfig::default();
    ipstack_config.mtu = MTU;
    ipstack_config.tcp_timeout = std::time::Duration::from_secs(args.tcp_timeout);
    ipstack_config.udp_timeout = std::time::Duration::from_secs(args.udp_timeout);

    let mut vpn_stream = UdpStream::connect(args.vpn_addr).await?;
    let buf: [u8; 1] = [0];
    vpn_stream.write_all(&buf).await?;

    let (stack, mut tcp_listener, udp_socket) = ::lwip::NetStack::new().unwrap();
    let (mut stack_sink, mut stack_stream) = stack.split();

    // tun device is assumed implementing `Stream` and `Sink`
    let (mut tun_sink, mut tun_stream) = tun.split();

    // Reads packet from stack and sends to TUN.
    tokio::spawn(async move {
        while let Some(pkt) = stack_stream.next().await {
            if let Ok(pkt) = pkt {
                tun_sink.send(pkt).await.unwrap();
            }
        }
    });

    // Reads packet from TUN and sends to stack.
    tokio::spawn(async move {
        while let Some(pkt) = tun_stream.next().await {
            if let Ok(pkt) = pkt {
                stack_sink.send(pkt).await.unwrap();
            }
        }
    });

    // Extracts TCP connections from stack and sends them to the dispatcher.
    tokio::spawn(async move {
        while let Some((stream, local_addr, remote_addr)) = tcp_listener.next().await {
            tokio::spawn(handle_inbound_stream(stream, local_addr, remote_addr));
        }
    });

    // Receive and send UDP packets between netstack and NAT manager. The NAT
    // manager would maintain UDP sessions and send them to the dispatcher.
    tokio::spawn(async move {
        handle_inbound_datagram(udp_socket).await;
    });

    Ok(())
}
