//! The emulator's debug link: a person's hands and eyes, for tests.
//!
//! Debug firmware (the emulator, never a shipped Trezor) listens on a second port where a test
//! can read the screen, press its buttons and learn the pairing code. Messages travel in Trezor's
//! older framing (`?##`, a type, a length, then the body in 64-byte packets), unencrypted.

use super::proto::{self, kind};
use anyhow::{Context, Result, bail, ensure};
use std::time::Duration;
use tokio::{net::UdpSocket, time::Instant};

const PACKET: usize = 64;
/// The right-hand button of a Safe 7's action bar, which turns a review's page.
const NEXT_PAGE: (u32, u32) = (315, 473);

pub struct DebugLink {
    socket: UdpSocket,
}

impl DebugLink {
    /// `port` is the emulator's wire port; its debug link is the next one.
    pub async fn connect(port: u16) -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        socket
            .connect(("127.0.0.1", port + 1))
            .await
            .context("could not reach the emulator's debug link")?;
        Ok(Self { socket })
    }

    async fn write(&mut self, kind: u16, body: &[u8]) -> Result<()> {
        let mut message = b"##".to_vec();
        message.extend_from_slice(&kind.to_be_bytes());
        message.extend_from_slice(&(body.len() as u32).to_be_bytes());
        message.extend_from_slice(body);
        for chunk in message.chunks(PACKET - 1) {
            let mut packet = vec![b'?'];
            packet.extend_from_slice(chunk);
            packet.resize(PACKET, 0);
            self.socket.send(&packet).await?;
        }
        Ok(())
    }

    async fn packet(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let mut packet = vec![0; PACKET];
        let received = tokio::time::timeout(timeout, self.socket.recv(&mut packet))
            .await
            .context("the emulator's debug link did not answer")??;
        ensure!(
            received == PACKET && packet[0] == b'?',
            "malformed debug packet"
        );
        Ok(packet)
    }

    async fn read(&mut self, timeout: Duration) -> Result<(u16, Vec<u8>)> {
        let first = self.packet(timeout).await?;
        ensure!(&first[1..3] == b"##", "malformed debug message");
        let kind = u16::from_be_bytes([first[3], first[4]]);
        let length = u32::from_be_bytes([first[5], first[6], first[7], first[8]]) as usize;
        let mut body = first[9..].to_vec();
        while body.len() < length {
            body.extend_from_slice(&self.packet(timeout).await?[1..]);
        }
        body.truncate(length);
        Ok((kind, body))
    }

    /// The screen now, as the firmware describes it: a JSON tree of its components.
    pub async fn layout(&mut self) -> Result<String> {
        self.write(
            kind::DEBUG_GET_STATE,
            &proto::encode(&proto::DebugGetState {
                wait_layout: Some(0),
            }),
        )
        .await?;
        let (answer, body) = self.read(Duration::from_secs(10)).await?;
        ensure!(
            answer == kind::DEBUG_STATE,
            "the debug link answered with message {answer}"
        );
        Ok(proto::decode::<proto::DebugState>(&body)?.tokens.concat())
    }

    /// The words on the screen now. Empty while no screen is up.
    pub async fn screen(&mut self) -> Result<String> {
        let layout = self.layout().await?;
        let mut words = String::new();
        if let Ok(layout) = serde_json::from_str(&layout) {
            collect_text(&layout, &mut words);
        }
        Ok(words)
    }

    /// Wait until the screen shows `text`, and return everything on it.
    pub async fn wait_for(&mut self, text: &str) -> Result<String> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let screen = self.screen().await?;
            if shows(&screen, text) {
                return Ok(screen);
            }
            if Instant::now() > deadline {
                bail!("the Trezor never showed {text:?}; its screen says {screen:?}");
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    /// Read a review that may run over several pages, from the page showing `text` to the last,
    /// as a person would: by touching the button that turns the page.
    pub async fn read_review(&mut self, text: &str) -> Result<String> {
        let mut review = self.wait_for(text).await?;
        let pages = serde_json::from_str::<serde_json::Value>(&self.layout().await?)
            .ok()
            .and_then(|layout| layout["page_count"].as_u64())
            .unwrap_or(1);
        for _ in 1..pages {
            let (x, y) = NEXT_PAGE;
            self.write(
                kind::DEBUG_DECISION,
                &proto::encode(&proto::DebugDecision {
                    button: None,
                    x: Some(x),
                    y: Some(y),
                }),
            )
            .await?;
            // The page turns with a short animation.
            let turned = Instant::now() + Duration::from_secs(5);
            let page = loop {
                tokio::time::sleep(Duration::from_millis(40)).await;
                let page = self.screen().await?;
                if !page.is_empty() && !review.ends_with(&page) {
                    break page;
                }
                if Instant::now() > turned {
                    bail!("the review did not turn its page; so far it says {review:?}");
                }
            };
            review.push(' ');
            review.push_str(&page);
        }
        Ok(review)
    }

    /// Approve or reject whatever the screen asks.
    pub async fn press(&mut self, approve: bool) -> Result<()> {
        self.write(
            kind::DEBUG_DECISION,
            &proto::encode(&proto::DebugDecision {
                button: Some(i32::from(approve)),
                x: None,
                y: None,
            }),
        )
        .await
    }

    /// The six-digit code the Trezor is showing for a pairing on this channel.
    pub async fn pairing_code(
        &mut self,
        channel: [u8; 2],
        handshake_hash: &[u8],
    ) -> Result<String> {
        self.write(
            kind::DEBUG_GET_PAIRING_INFO,
            &proto::encode(&proto::DebugGetPairingInfo {
                channel_id: Some(channel.to_vec()),
                handshake_hash: Some(handshake_hash.to_vec()),
            }),
        )
        .await?;
        let (answer, body) = self.read(Duration::from_secs(10)).await?;
        ensure!(
            answer == kind::DEBUG_PAIRING_INFO,
            "the debug link answered with message {answer}"
        );
        let code = proto::decode::<proto::DebugPairingInfo>(&body)?
            .code_entry_code
            .context("the Trezor is not showing a pairing code")?;
        Ok(format!("{code:06}"))
    }
}

/// Every piece of text in a layout, in the order the firmware lists it. A component's own name
/// is not text on the screen.
fn collect_text(layout: &serde_json::Value, words: &mut String) {
    match layout {
        serde_json::Value::String(text) => words.push_str(text),
        serde_json::Value::Array(parts) => {
            for part in parts {
                collect_text(part, words);
            }
            words.push(' ');
        }
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                if name != "component" {
                    collect_text(value, words);
                    if value.is_string() {
                        words.push(' ');
                    }
                }
            }
        }
        _ => {}
    }
}

/// Whether a screen's words include `text`. The screen breaks long words over lines, with a
/// hyphen where it is prose, so line breaks, spaces and hyphens are not compared.
pub fn shows(screen: &str, text: &str) -> bool {
    let plain = |text: &str| -> String {
        text.chars()
            .filter(|character| !character.is_whitespace() && *character != '-')
            .collect()
    };
    plain(screen).contains(&plain(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layouts_words_are_found_across_its_line_breaks() {
        let layout = serde_json::json!({
            "component": "TextScreen",
            "Header": { "component": "Header", "title": { "component": "Label", "text": "Before you continue" } },
            "Content": { "component": "FormattedText", "text": ["Allow ", "Anzen", " on ", "Ph", "-", "\n", "one", " to pair with this", "\n", "Trezor?"] },
            "page_count": 1,
        });
        let mut words = String::new();
        collect_text(&layout, &mut words);
        assert!(
            shows(&words, "Allow Anzen on Phone to pair with this Trezor?"),
            "{words}"
        );
        assert!(shows(&words, "Before you continue"));
        assert!(!shows(&words, "TextScreen"));
        assert!(!shows(&words, "Set up vault"));
    }
}
