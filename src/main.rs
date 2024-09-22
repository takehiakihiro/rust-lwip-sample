use anyhow::Result;
use clap::Parser;
use futures::{SinkExt, StreamExt};
use lwip::NetStack;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    os::fd::AsRawFd,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tun::Tun;
// use udp_stream::UdpStream;

// const MTU: u16 = 1400;

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
    // /// echo server address, likes `127.0.0.1:4000`
    // #[arg(short, long, value_name = "IP:port", default_value = "127.0.0.1:4000")]
    // vpn_addr: SocketAddr,

    // /// echo server address, likes `127.0.0.1`
    // #[arg(short, long, value_name = "IP address", default_value = "127.0.0.1")]
    // server_addr: IpAddr,
    /// Verbosity level
    #[arg(short, long, value_name = "level", value_enum, default_value = "info")]
    pub verbosity: ArgVerbosity,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let default = format!("{:?}", args.verbosity);
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default)).init();

    // TUNデバイスの作成
    let tun = Tun::builder()
        .address(Ipv4Addr::new(10, 0, 0, 1))
        .netmask(Ipv4Addr::new(255, 255, 255, 0))
        .up() // or set it up manually using `sudo ip link set <tun-name> up`.
        .try_build() // or `.try_build_mq(queues)` for multi-queue support.
        .unwrap();

    println!("tun created, name: {}, fd: {}", tun.name(), tun.as_raw_fd());

    let (mut tun_reader, mut tun_writer) = tokio::io::split(tun);

    // let mut vpn_stream = UdpStream::connect(args.vpn_addr).await?;
    // let buf: [u8; 1] = [0];
    // vpn_stream.write_all(&buf).await?;

    let (stack, mut tcp_listener, udp_socket) = NetStack::new()?;
    let (mut stack_sink, mut stack_stream) = stack.split();

    // Reads packet from stack and sends to TUN.
    let stack_reader_task = tokio::spawn(async move {
        while let Some(pkt) = stack_stream.next().await {
            if let Ok(pkt) = pkt {
                tun_writer.write(&pkt).await.unwrap();
            }
        }
    });

    // Reads packet from TUN and sends to stack.
    let stack_writer_task = tokio::spawn(async move {
        loop {
            let mut pkt = [0u8; 4096];
            let n = match tun_reader.read(&mut pkt).await {
                Ok(v) => v,
                Err(e) => {
                    return;
                }
            };
            stack_sink.send(pkt[..n].to_vec()).await.unwrap();
        }
    });

    // Extracts TCP connections from stack and sends them to the dispatcher.
    let tcp_listener_task = tokio::spawn(async move {
        while let Some((stream, local_addr, remote_addr)) = tcp_listener.next().await {
            tokio::spawn(handle_inbound_stream(stream, local_addr, remote_addr));
        }
    });

    // Receive and send UDP packets between netstack and NAT manager. The NAT
    // manager would maintain UDP sessions and send them to the dispatcher.
    let udp_listener_task = tokio::spawn(async move { handle_inbound_datagram(udp_socket).await });

    let _ = tokio::join!(
        stack_reader_task,
        stack_writer_task,
        tcp_listener_task,
        udp_listener_task
    );

    Ok(())
}

///
async fn handle_inbound_stream(
    stream: lwip::TcpStream,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
) -> Result<()> {
    log::info!("accepted tcp {} -> {}", local_addr, remote_addr);
    Ok(())
}

///
async fn handle_inbound_datagram(mut udp_socket: Box<lwip::UdpSocket>) -> Result<()> {
    while let Some((pkt, local_addr, remote_addr)) = udp_socket.next().await {
        log::info!(
            "recved udp {} -> {}, len={}",
            local_addr,
            remote_addr,
            pkt.len()
        );
    }

    Ok(())
}
