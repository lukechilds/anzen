//! Ledger's Speculos emulator, over its APDU TCP port: for testing the app without a device.

use super::{Answer, Exchange};
use anyhow::{Context, Result, anyhow};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub struct Speculos(TcpStream);

impl Speculos {
    pub async fn connect(address: &str) -> Result<Self> {
        Ok(Self(TcpStream::connect(address).await.with_context(
            || format!("no Speculos emulator at {address}"),
        )?))
    }
}

impl Exchange for Speculos {
    /// Each direction is length-prefixed; the answer's length leaves out its status word.
    async fn exchange(&mut self, apdu: &[u8], timeout: Duration) -> Result<Answer> {
        self.0.write_all(&(apdu.len() as u32).to_be_bytes()).await?;
        self.0.write_all(apdu).await?;
        let answer = async {
            let mut length = [0_u8; 4];
            self.0.read_exact(&mut length).await?;
            let mut answer = vec![0_u8; u32::from_be_bytes(length) as usize + 2];
            self.0.read_exact(&mut answer).await?;
            anyhow::Ok(answer)
        };
        let answer = tokio::time::timeout(timeout, answer)
            .await
            .map_err(|_| anyhow!("the emulator did not answer in time"))??;
        Answer::parse(answer)
    }
}
