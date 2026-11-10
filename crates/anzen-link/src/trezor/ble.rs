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
//! Written from Trezor's host library (`trezorlib.transport.ble`). The emulator speaks THP over
//! UDP, so no automated test reaches this file; it has been run by hand against a Safe 7 from a
//! Mac (the `trezor` example), which is where what it says about advertisements comes from.

use super::Transport;
use anyhow::{Context, Result, bail};
use btleplug::{
    api::{
        Central, Characteristic, Manager as _, Peripheral as _, PeripheralProperties, ScanFilter,
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

/// The company identifier in a Trezor's manufacturer data.
const COMPANY: u16 = 0x0f29;
/// How long a device that only might be a Trezor gets to show that it is one.
const PROBE: Duration = Duration::from_secs(12);
/// How long the person has to confirm a first Bluetooth pairing on both devices.
const BONDING: Duration = Duration::from_secs(120);

/// A device seen while scanning that is, or may be, a Trezor.
pub struct TrezorFound {
    /// The name the device advertises: its label, which its owner chose.
    pub name: String,
    /// Whether what it advertises says it is a Trezor. Otherwise it only could be one, and
    /// connecting finds out.
    pub certain: bool,
    peripheral: Peripheral,
}

/// What an advertisement says about its sender.
///
/// A Trezor's advertisement proper is its name and nothing else. Its service and its company's
/// manufacturer data follow in the scan response, which a host does not always get: not from a
/// Trezor outside pairing mode that it has not bonded with, and (seen with a Safe 7 and a Mac)
/// not reliably even then. So a device that names its service or its company is a Trezor, and
/// one that advertises a bare name may be.
fn likeness(properties: &PeripheralProperties) -> Option<bool> {
    if properties.services.contains(&SERVICE) || properties.manufacturer_data.contains_key(&COMPANY)
    {
        return Some(true);
    }
    let bare_name = properties
        .local_name
        .as_deref()
        .is_some_and(|name| !name.is_empty())
        && properties.services.is_empty()
        && properties.manufacturer_data.is_empty()
        && properties.service_data.is_empty();
    bare_name.then_some(false)
}

/// Devices advertising nearby, for `window`, that are or may be Trezors: the certain ones first.
pub async fn scan(window: Duration) -> Result<Vec<TrezorFound>> {
    let manager = Manager::new().await.context("Bluetooth is unavailable")?;
    let central = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .context("this device has no Bluetooth adapter")?;
    // Unfiltered: Trezor's own library finds scanning by service unreliable too.
    central
        .start_scan(ScanFilter::default())
        .await
        .context("could not scan for Bluetooth devices: check Bluetooth is on and allowed")?;
    tokio::time::sleep(window).await;
    central.stop_scan().await.ok();
    let mut found = Vec::new();
    for peripheral in central.peripherals().await? {
        let Some(properties) = peripheral.properties().await? else {
            continue;
        };
        let Some(certain) = likeness(&properties) else {
            continue;
        };
        found.push(TrezorFound {
            name: properties
                .local_name
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "Trezor".to_owned()),
            certain,
            peripheral,
        });
    }
    found.sort_by_key(|trezor| !trezor.certain);
    Ok(found)
}

/// Find a Trezor within `window` (the one whose name contains `name`, if given) and connect to
/// it. A device that only might be a Trezor is connected to and asked; one that is not is left
/// alone after that.
pub async fn open(name: Option<&str>, window: Duration) -> Result<Bluetooth> {
    let started = tokio::time::Instant::now();
    let mut not_trezors = Vec::new();
    // Why each device that might have been a Trezor was passed over, for the error.
    let mut refusals = Vec::new();
    // Short scans, so the first match connects without waiting out the window.
    while started.elapsed() < window {
        for found in scan(Duration::from_secs(3)).await? {
            if name.is_some_and(|name| !found.name.contains(name))
                || not_trezors.contains(&found.peripheral.id())
            {
                continue;
            }
            if found.certain {
                return found.connect().await;
            }
            let (id, name) = (found.peripheral.id(), found.name.clone());
            match tokio::time::timeout(PROBE, found.attach()).await {
                // It is a Trezor. From here the person may be asked to confirm the pairing.
                Ok(Ok(attached)) => return attached.bond().await,
                Ok(Err(error)) => {
                    refusals.push(format!("{name}: {error:#}"));
                    not_trezors.push(id);
                }
                // No answer says nothing about what it is: a Trezor outside pairing mode
                // ignores a host it has not bonded with. It is tried again on the next pass.
                Err(_) => {
                    let silent = format!("{name} did not answer");
                    if !refusals.contains(&silent) {
                        refusals.push(silent);
                    }
                }
            }
        }
    }
    let tried = if refusals.is_empty() {
        String::new()
    } else {
        format!(" (Tried {}.)", refusals.join("; "))
    };
    bail!(
        "No Trezor found. Unlock it and keep it nearby. The first time, put it in pairing mode: on the Trezor, open Pair & Connect and choose Pair new device.{tried}"
    )
}

/// An open Bluetooth link to a Trezor.
pub struct Bluetooth {
    peripheral: Peripheral,
    write: Characteristic,
    notifications: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
    pub name: String,
}

/// A connected device that has shown it is a Trezor, not yet bonded with.
struct Attached {
    name: String,
    peripheral: Peripheral,
    notify: Characteristic,
    write: Characteristic,
}

impl TrezorFound {
    /// Connect and look for the Trezor's service. Asks nothing of the person.
    async fn attach(self) -> Result<Attached> {
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
        };
        let (Some(notify), Some(write)) = (characteristic(TX), characteristic(RX)) else {
            peripheral.disconnect().await.ok();
            bail!("{} is not a Trezor", self.name);
        };
        Ok(Attached {
            name: self.name,
            peripheral,
            notify,
            write,
        })
    }

    /// Connect. The first time, the operating system and the Trezor each show a code to confirm.
    pub async fn connect(self) -> Result<Bluetooth> {
        self.attach().await?.bond().await
    }
}

impl Attached {
    /// Subscribe to the Trezor's packets, which is what triggers Bluetooth pairing on a new
    /// device: this waits while the person confirms the code on both.
    async fn bond(self) -> Result<Bluetooth> {
        let subscribed = tokio::time::timeout(BONDING, self.peripheral.subscribe(&self.notify));
        match subscribed.await {
            Ok(Ok(())) => {}
            failed => {
                self.peripheral.disconnect().await.ok();
                failed
                    .map_err(|_| anyhow::anyhow!("the pairing was not confirmed in time"))?
                    .context(
                        "the Trezor refused the link: confirm the pairing code on both devices",
                    )?;
            }
        }
        Ok(Bluetooth {
            notifications: self.peripheral.notifications().await?,
            peripheral: self.peripheral,
            write: self.write,
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
    fn an_advertisement_says_whether_its_sender_is_or_may_be_a_trezor() {
        let advert =
            |name: Option<&str>, services: &[Uuid], company: Option<u16>| PeripheralProperties {
                local_name: name.map(str::to_owned),
                services: services.to_vec(),
                manufacturer_data: company
                    .map(|company| (company, vec![0; 6]))
                    .into_iter()
                    .collect(),
                ..Default::default()
            };
        // Its service or its company's data: a Trezor, whatever its owner named it.
        assert_eq!(
            likeness(&advert(Some("Savings"), &[SERVICE], None)),
            Some(true)
        );
        assert_eq!(likeness(&advert(None, &[], Some(COMPANY))), Some(true));
        // A bare name is all a Trezor's advertisement proper holds: it may be one.
        assert_eq!(likeness(&advert(Some("Anzen Dev"), &[], None)), Some(false));
        // Anything else is something else.
        let heart_rate = Uuid::from_u128(0x0000180d_0000_1000_8000_00805f9b34fb);
        assert_eq!(likeness(&advert(Some("Strap"), &[heart_rate], None)), None);
        assert_eq!(likeness(&advert(Some("iPhone"), &[], Some(76))), None);
        assert_eq!(likeness(&advert(None, &[], None)), None);
        assert_eq!(likeness(&advert(Some(""), &[], None)), None);
    }

    #[test]
    fn the_characteristics_are_trezors() {
        assert_eq!(SERVICE.to_string(), "8c000001-a59b-4d58-a9ad-073df69fa1b1");
        assert_eq!(RX.to_string(), "8c000002-a59b-4d58-a9ad-073df69fa1b1");
        assert_eq!(TX.to_string(), "8c000003-a59b-4d58-a9ad-073df69fa1b1");
    }
}
