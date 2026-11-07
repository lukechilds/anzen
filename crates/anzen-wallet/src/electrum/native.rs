//! Native WebSocket connections: `tungstenite` over TCP, with rustls and Mozilla's root
//! certificates for `wss://`. Socket mechanics only; Electrum itself is in the parent module.

use super::{Connector, FrameTransport, MAX_FRAME_BYTES};
use anyhow::{Context, Result, bail};
use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};
use tungstenite::{Message, WebSocket, client::IntoClientRequest, protocol::WebSocketConfig};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Default)]
pub struct WebSocketConnector;

impl Connector for WebSocketConnector {
    fn connect(&self, url: &str) -> Result<Box<dyn FrameTransport>> {
        Ok(Box::new(NativeSocket::connect(url)?))
    }
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(tcp) => tcp,
            Self::Tls(tls) => tls.get_ref(),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.read(buf),
            Self::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(tcp) => tcp.write(buf),
            Self::Tls(tls) => tls.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(tcp) => tcp.flush(),
            Self::Tls(tls) => tls.flush(),
        }
    }
}

pub struct NativeSocket {
    socket: WebSocket<Stream>,
}

impl NativeSocket {
    pub fn connect(url: &str) -> Result<Self> {
        let request = url
            .into_client_request()
            .with_context(|| format!("invalid Electrum WebSocket URL {url}"))?;
        let uri = request.uri().clone();
        let secure = match uri.scheme_str() {
            Some("wss") => true,
            Some("ws") => false,
            _ => bail!("Electrum server {url} must be a WebSocket (wss://) URL"),
        };
        let host = uri.host().context("Electrum URL has no host")?.to_owned();
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let address = (host.as_str(), port)
            .to_socket_addrs()
            .with_context(|| format!("could not resolve {host}"))?
            .next()
            .with_context(|| format!("{host} has no address"))?;
        let tcp = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
            .with_context(|| format!("could not connect to {url}"))?;
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(REQUEST_TIMEOUT))?;
        tcp.set_write_timeout(Some(REQUEST_TIMEOUT))?;
        let stream = if secure {
            let name = rustls::pki_types::ServerName::try_from(host.clone())
                .with_context(|| format!("invalid TLS server name {host}"))?;
            let connection = rustls::ClientConnection::new(tls_config()?, name)?;
            Stream::Tls(Box::new(rustls::StreamOwned::new(connection, tcp)))
        } else {
            Stream::Plain(tcp)
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_FRAME_BYTES))
            .max_frame_size(Some(MAX_FRAME_BYTES));
        let (socket, _) = tungstenite::client::client_with_config(request, stream, Some(config))
            .map_err(|error| anyhow::anyhow!("WebSocket handshake with {url} failed: {error}"))?;
        Ok(Self { socket })
    }

    /// Waits up to `timeout` for a frame: `Ok(None)` if none arrived, an error if the
    /// connection is gone. Used by the chain watcher, which also needs to run its timers.
    pub fn receive_within(&mut self, timeout: Duration) -> Result<Option<String>> {
        self.socket
            .get_ref()
            .tcp()
            .set_read_timeout(Some(timeout))?;
        let result = self.next_frame();
        self.socket
            .get_ref()
            .tcp()
            .set_read_timeout(Some(REQUEST_TIMEOUT))?;
        match result {
            Err(Failure::TimedOut) => Ok(None),
            Err(Failure::Other(error)) => Err(error),
            Ok(frame) => Ok(Some(frame)),
        }
    }

    fn next_frame(&mut self) -> std::result::Result<String, Failure> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => return Ok(text.as_str().to_owned()),
                Ok(Message::Binary(bytes)) => {
                    return String::from_utf8(bytes.to_vec()).map_err(|_| {
                        Failure::Other(anyhow::anyhow!("Electrum frame is not UTF-8"))
                    });
                }
                // tungstenite answers pings itself.
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Close(_)) => {
                    return Err(Failure::Other(anyhow::anyhow!(
                        "Electrum WebSocket disconnected"
                    )));
                }
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Err(Failure::TimedOut);
                }
                Err(error) => return Err(Failure::Other(error.into())),
            }
        }
    }
}

enum Failure {
    TimedOut,
    Other(anyhow::Error),
}

impl FrameTransport for NativeSocket {
    fn send(&mut self, frame: &str) -> Result<()> {
        self.socket.send(Message::text(frame))?;
        Ok(())
    }
    fn receive(&mut self) -> Result<String> {
        match self.next_frame() {
            Ok(frame) => Ok(frame),
            Err(Failure::TimedOut) => bail!("Electrum request timed out"),
            Err(Failure::Other(error)) => Err(error),
        }
    }
}

impl Drop for NativeSocket {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
    }
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::new(config))
}
