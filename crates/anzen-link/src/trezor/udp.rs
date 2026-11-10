//! The Trezor emulator's wire: one THP packet per UDP datagram.

use super::Transport;
use anyhow::{Context, Result};
use std::time::Duration;
use tokio::net::UdpSocket;

/// The emulator's default port. Each emulator listens on its own; the debug link is one above.
pub const DEFAULT_PORT: u16 = 21324;
const PACKET: usize = 64;

pub struct Emulator {
    socket: UdpSocket,
}

impl Emulator {
    pub async fn connect(port: u16) -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        socket
            .connect(("127.0.0.1", port))
            .await
            .context("could not reach the Trezor emulator")?;
        Ok(Self { socket })
    }
}

impl Transport for Emulator {
    fn packet_len(&self) -> usize {
        PACKET
    }

    async fn send(&mut self, packet: &[u8]) -> Result<()> {
        self.socket
            .send(packet)
            .await
            .context("the Trezor emulator is not running")?;
        Ok(())
    }

    async fn receive(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let mut packet = vec![0; PACKET];
        match tokio::time::timeout(timeout, self.socket.recv(&mut packet)).await {
            Ok(received) => {
                packet.truncate(received.context("the Trezor emulator is not running")?);
                Ok(Some(packet))
            }
            Err(_) => Ok(None),
        }
    }
}
