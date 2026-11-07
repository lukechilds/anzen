//! Browser WebSocket connections. JavaScript in the device's I/O worker owns the socket and
//! passes whole frames; no keys, descriptors or signing requests are sent to it.

use super::{Connector, FrameTransport};
use anyhow::Result;
use anzen_common::storage::browser;
use serde_json::json;

#[derive(Clone, Copy, Default)]
pub struct BrowserConnector;

impl Connector for BrowserConnector {
    fn connect(&self, url: &str) -> Result<Box<dyn FrameTransport>> {
        let session = browser::call(&json!({"type":"electrum", "operation":"open", "url":url}))?;
        Ok(Box::new(BrowserSocket { session }))
    }
}

struct BrowserSocket {
    session: u64,
}

impl FrameTransport for BrowserSocket {
    fn send(&mut self, frame: &str) -> Result<()> {
        browser::call(
            &json!({"type":"electrum", "operation":"send", "session":self.session, "frame":frame}),
        )
    }
    fn receive(&mut self) -> Result<String> {
        browser::call(&json!({"type":"electrum", "operation":"receive", "session":self.session}))
    }
}

impl Drop for BrowserSocket {
    fn drop(&mut self) {
        let _ = browser::call::<()>(
            &json!({"type":"electrum", "operation":"close", "session":self.session}),
        );
    }
}
