//! Finding and talking to a Ledger with `btleplug`.

use super::{Answer, frame};
use anyhow::{Context, Result, anyhow, bail};
use btleplug::{
    api::{
        Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, ScanFilter,
        ValueNotification, WriteType,
    },
    platform::{Manager, Peripheral},
};
use futures::{Stream, StreamExt};
use std::{pin::Pin, time::Duration};
use uuid::Uuid;

/// Every Ledger model has its own service, `13d63400-2c97-MMMM-0000-4c6564676572`, where `MMMM`
/// names the model. Its characteristics replace the `0000`: `0001` notifies, `0002` is written.
const SERVICE_MASK: u128 = 0xffffffff_ffff_0000_ffff_ffffffffffff;
const SERVICE_BASE: u128 = 0x13d63400_2c97_0000_0000_4c6564676572;
const NOTIFY: u128 = 0x0001 << 48;
const WRITE: u128 = 0x0002 << 48;

/// How long one command may take. Reviews wait for a person, so callers pass their own.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

fn ledger_service(uuid: &Uuid) -> bool {
    uuid.as_u128() & SERVICE_MASK == SERVICE_BASE
}

fn model(service: &Uuid) -> &'static str {
    match (service.as_u128() >> 64) as u16 {
        0x0004 => "Nano X",
        0x6004 => "Stax",
        0x3004 => "Flex",
        _ => "Ledger",
    }
}

/// A Ledger seen while scanning.
pub struct LedgerFound {
    /// The name the device advertises, e.g. `Ledger Flex 1A2B`.
    pub name: String,
    pub model: &'static str,
    service: Uuid,
    peripheral: Peripheral,
}

/// Ledgers advertising nearby, for `window`. A device already connected to another host does
/// not advertise.
pub async fn scan(window: Duration) -> Result<Vec<LedgerFound>> {
    let manager = Manager::new().await.context("Bluetooth is unavailable")?;
    let central = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .context("this device has no Bluetooth adapter")?;
    let mut events = central.events().await?;
    central
        .start_scan(ScanFilter::default())
        .await
        .context("could not scan for Bluetooth devices: check Bluetooth is on and allowed")?;
    let mut found: Vec<LedgerFound> = Vec::new();
    let deadline = tokio::time::sleep(window);
    tokio::pin!(deadline);
    loop {
        let event = tokio::select! {
            () = &mut deadline => break,
            event = events.next() => event,
        };
        let id = match event {
            Some(
                CentralEvent::DeviceDiscovered(id)
                | CentralEvent::DeviceUpdated(id)
                | CentralEvent::ServicesAdvertisement { id, .. },
            ) => id,
            Some(_) => continue,
            None => break,
        };
        let peripheral = central.peripheral(&id).await?;
        if found.iter().any(|ledger| ledger.peripheral.id() == id) {
            continue;
        }
        let Some(properties) = peripheral.properties().await? else {
            continue;
        };
        let Some(service) = properties.services.iter().copied().find(ledger_service) else {
            continue;
        };
        found.push(LedgerFound {
            name: properties.local_name.unwrap_or_else(|| "Ledger".to_owned()),
            model: model(&service),
            service,
            peripheral,
        });
    }
    central.stop_scan().await.ok();
    Ok(found)
}

/// The first Ledger seen within `window`, or the one whose name contains `name`.
pub async fn find(name: Option<&str>, window: Duration) -> Result<LedgerFound> {
    let started = tokio::time::Instant::now();
    // Short scans, so the first match returns without waiting out the window.
    while started.elapsed() < window {
        let found = scan(Duration::from_secs(2)).await?;
        if let Some(ledger) = found
            .into_iter()
            .find(|ledger| name.is_none_or(|name| ledger.name.contains(name)))
        {
            return Ok(ledger);
        }
    }
    bail!("no Ledger found: unlock it, turn its Bluetooth on and keep it nearby")
}

/// An open link to a Ledger. One command at a time.
pub struct Ledger {
    peripheral: Peripheral,
    write: Characteristic,
    notifications: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    notify: Uuid,
    mtu: usize,
    pub name: String,
    pub model: &'static str,
}

impl LedgerFound {
    /// Connect. The first time, the operating system and the Ledger each show a pairing code to
    /// confirm.
    pub async fn connect(self) -> Result<Ledger> {
        let peripheral = self.peripheral;
        peripheral
            .connect()
            .await
            .context("could not connect to the Ledger")?;
        peripheral.discover_services().await?;
        let base = self.service.as_u128();
        let characteristic = |offset: u128| {
            let uuid = Uuid::from_u128(base | offset);
            peripheral
                .characteristics()
                .into_iter()
                .find(|characteristic| characteristic.uuid == uuid)
                .ok_or_else(|| anyhow!("the Ledger lacks its characteristic {uuid}"))
        };
        let (notify, write) = (characteristic(NOTIFY)?, characteristic(WRITE)?);
        // Subscribing is what triggers pairing on a new device.
        peripheral
            .subscribe(&notify)
            .await
            .context("the Ledger refused the link: confirm the pairing code on both devices")?;
        let mut ledger = Ledger {
            notifications: peripheral.notifications().await?,
            peripheral,
            write,
            notify: notify.uuid,
            mtu: frame::DEFAULT_MTU,
            name: self.name,
            model: self.model,
        };
        ledger.negotiate_mtu().await?;
        Ok(ledger)
    }
}

impl Ledger {
    async fn send(&mut self, frame: &[u8]) -> Result<()> {
        self.peripheral
            .write(&self.write, frame, WriteType::WithResponse)
            .await
            .context("could not write to the Ledger")
    }

    async fn next_frame(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        loop {
            let notification = tokio::time::timeout(timeout, self.notifications.next())
                .await
                .map_err(|_| anyhow!("the Ledger did not answer in time"))?
                .context("the Ledger disconnected")?;
            if notification.uuid == self.notify {
                return Ok(notification.value);
            }
        }
    }

    async fn negotiate_mtu(&mut self) -> Result<()> {
        self.send(&frame::MTU_REQUEST).await?;
        let answer = self.next_frame(DEFAULT_TIMEOUT).await?;
        let mtu = frame::mtu_answer(&answer).context("the Ledger did not report its frame size")?;
        if mtu > frame::DEFAULT_MTU {
            self.mtu = mtu;
        }
        Ok(())
    }

    /// Whether the link is still up, as far as the operating system knows.
    pub async fn is_connected(&self) -> bool {
        self.peripheral.is_connected().await.unwrap_or(false)
    }

    /// The frame size the device accepts.
    pub fn mtu(&self) -> usize {
        self.mtu
    }

    pub async fn disconnect(self) -> Result<()> {
        self.peripheral.disconnect().await?;
        Ok(())
    }
}

impl super::Exchange for Ledger {
    /// Send one APDU to the open app and wait up to `timeout` for its answer.
    async fn exchange(&mut self, apdu: &[u8], timeout: Duration) -> Result<Answer> {
        for frame in frame::frames(apdu, self.mtu)? {
            self.send(&frame).await?;
        }
        let mut reassembly = frame::Reassembly::default();
        loop {
            let frame = self.next_frame(timeout).await?;
            if let Some(answer) = reassembly.push(&frame)? {
                return Answer::parse(answer);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_services_are_recognised_by_their_model() {
        let flex = Uuid::parse_str("13d63400-2c97-3004-0000-4c6564676572").unwrap();
        assert!(ledger_service(&flex));
        assert_eq!(model(&flex), "Flex");
        assert_eq!(
            Uuid::from_u128(flex.as_u128() | WRITE).to_string(),
            "13d63400-2c97-3004-0002-4c6564676572"
        );
        let heart_rate = Uuid::parse_str("0000180d-0000-1000-8000-00805f9b34fb").unwrap();
        assert!(!ledger_service(&heart_rate));
    }
}
