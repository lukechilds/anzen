//! Finding and talking to a Trezor over Bluetooth with `btleplug`.
//!
//! A Trezor Safe 7 advertises one service with two characteristics: the host writes THP packets
//! to one and is notified of the Trezor's on the other, 244 bytes each. The first connection
//! bonds the two at the Bluetooth level (the operating system and the Trezor each show a code);
//! THP's own pairing then happens inside the link.
//!
//! The service is named in the Trezor's scan response, which it sends only to hosts it has bonded
//! with, unless it is in pairing mode ("Pair & Connect", then "Pair new device", on the Trezor).
//! To a host it has not bonded with, a Trezor outside pairing mode is a name and nothing else,
//! and it accepts no connection from one. So a new host finds a Trezor only in pairing mode.
//!
//! Written from Trezor's host library (`trezorlib.transport.ble`) and not yet run against a
//! device: the emulator speaks THP over UDP, so everything above this file is tested and this
//! file is not.

use super::Transport;
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

const SERVICE: Uuid = Uuid::from_u128(0x8c000001_a59b_4d58_a9ad_073df69fa1b1);
/// Written by the host.
const RX: Uuid = Uuid::from_u128(0x8c000002_a59b_4d58_a9ad_073df69fa1b1);
/// Notified by the Trezor.
const TX: Uuid = Uuid::from_u128(0x8c000003_a59b_4d58_a9ad_073df69fa1b1);
const PACKET: usize = 244;

/// A Trezor seen while scanning.
pub struct TrezorFound {
    /// The name the device advertises.
    pub name: String,
    peripheral: Peripheral,
}

/// Trezors advertising nearby, for `window`.
pub async fn scan(window: Duration) -> Result<Vec<TrezorFound>> {
    let manager = Manager::new().await.context("Bluetooth is unavailable")?;
    let central = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .context("this device has no Bluetooth adapter")?;
    let mut events = central.events().await?;
    // Unfiltered: Trezor's own library finds scanning by service unreliable, and checks what
    // each device advertises itself.
    central
        .start_scan(ScanFilter::default())
        .await
        .context("could not scan for Bluetooth devices: check Bluetooth is on and allowed")?;
    let mut found: Vec<TrezorFound> = Vec::new();
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
        if found.iter().any(|trezor| trezor.peripheral.id() == id) {
            continue;
        }
        let Some(properties) = peripheral.properties().await? else {
            continue;
        };
        let name = properties.local_name.unwrap_or_default();
        if !is_trezor(&properties.services, &name) {
            continue;
        }
        found.push(TrezorFound {
            name: if name.is_empty() {
                "Trezor".to_owned()
            } else {
                name
            },
            peripheral,
        });
    }
    central.stop_scan().await.ok();
    Ok(found)
}

/// A Trezor advertises its service. One whose advertisement left the service out is still
/// recognised by the name it gives itself.
fn is_trezor(services: &[Uuid], name: &str) -> bool {
    services.contains(&SERVICE) || name.starts_with("Trezor")
}

/// The first Trezor seen within `window`, or the one whose name contains `name`.
pub async fn find(name: Option<&str>, window: Duration) -> Result<TrezorFound> {
    let started = tokio::time::Instant::now();
    // Short scans, so the first match returns without waiting out the window.
    while started.elapsed() < window {
        let found = scan(Duration::from_secs(2)).await?;
        if let Some(trezor) = found
            .into_iter()
            .find(|trezor| name.is_none_or(|name| trezor.name.contains(name)))
        {
            return Ok(trezor);
        }
    }
    bail!(
        "No Trezor found. Unlock it and keep it nearby. The first time, put it in pairing mode: on the Trezor, open Pair & Connect and choose Pair new device."
    )
}

/// An open Bluetooth link to a Trezor.
pub struct Bluetooth {
    peripheral: Peripheral,
    write: Characteristic,
    notifications: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    pub name: String,
}

impl TrezorFound {
    /// Connect. The first time, the operating system and the Trezor each show a code to confirm.
    pub async fn connect(self) -> Result<Bluetooth> {
        let peripheral = self.peripheral;
        peripheral
            .connect()
            .await
            .context("could not connect to the Trezor")?;
        peripheral.discover_services().await?;
        let characteristic = |uuid: Uuid| {
            peripheral
                .characteristics()
                .into_iter()
                .find(|characteristic| characteristic.uuid == uuid)
                .ok_or_else(|| anyhow!("the Trezor lacks its characteristic {uuid}"))
        };
        let (notify, write) = (characteristic(TX)?, characteristic(RX)?);
        // Subscribing is what triggers Bluetooth pairing on a new device.
        peripheral
            .subscribe(&notify)
            .await
            .context("the Trezor refused the link: confirm the pairing code on both devices")?;
        Ok(Bluetooth {
            notifications: peripheral.notifications().await?,
            peripheral,
            write,
            name: self.name,
        })
    }
}

impl Bluetooth {
    /// Whether the link is still up, as far as the operating system knows.
    pub async fn is_connected(&self) -> bool {
        self.peripheral.is_connected().await.unwrap_or(false)
    }

    pub async fn disconnect(self) -> Result<()> {
        self.peripheral.disconnect().await?;
        Ok(())
    }
}

impl Transport for Bluetooth {
    fn packet_len(&self) -> usize {
        PACKET
    }

    async fn send(&mut self, packet: &[u8]) -> Result<()> {
        // Trezor's own library writes with a response on Apple's Bluetooth stack and without one
        // elsewhere.
        let mode = if cfg!(target_vendor = "apple") {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        self.peripheral
            .write(&self.write, packet, mode)
            .await
            .context("could not write to the Trezor")
    }

    async fn receive(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        loop {
            match tokio::time::timeout(timeout, self.notifications.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => bail!("the Trezor disconnected"),
                Ok(Some(notification)) if notification.uuid == TX => {
                    return Ok(Some(notification.value));
                }
                Ok(Some(_)) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trezor_is_known_by_its_service_or_its_name() {
        assert!(is_trezor(&[SERVICE], ""));
        assert!(is_trezor(&[], "Trezor Safe 7"));
        let heart_rate = Uuid::from_u128(0x0000180d_0000_1000_8000_00805f9b34fb);
        assert!(!is_trezor(&[heart_rate], "Ledger Flex 1A2B"));
    }

    #[test]
    fn the_characteristics_are_trezors() {
        assert_eq!(SERVICE.to_string(), "8c000001-a59b-4d58-a9ad-073df69fa1b1");
        assert_eq!(RX.to_string(), "8c000002-a59b-4d58-a9ad-073df69fa1b1");
        assert_eq!(TX.to_string(), "8c000003-a59b-4d58-a9ad-073df69fa1b1");
    }
}
