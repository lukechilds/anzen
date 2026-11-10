//! The Anzen app's image, and loading it onto a Trezor.
//!
//! A Trezor keeps no apps installed: the host uploads the app when it connects, and the Trezor
//! runs it only if its header is proven to belong to a set of apps signed for that firmware (a
//! Merkle proof against a signed "root packet"). An image the Trezor still holds from an earlier
//! connection is not uploaded again.

use super::{
    Transport, Trezor,
    proto::{self, kind},
    thp::{QUICK, REVIEW},
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;

/// Everything a Trezor needs to load one build of an app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppImage {
    /// The image's fixed-size header: what the app is, what it may do and its first chunk's hash.
    header: Vec<u8>,
    /// The image in the pieces the Trezor asks for, each with the hash of the pieces after it.
    chunks: Vec<(Vec<u8>, Vec<u8>)>,
    /// The Merkle proof that the header is in the root packet's set of apps.
    proof: Vec<u8>,
    root_packet: Vec<u8>,
}

/// Offsets in the header (`AppHeader` in Trezor's `trezor-app-tool`).
mod header {
    use std::ops::Range;
    pub const MAGIC: &[u8] = b"TRZA";
    pub const LENGTH: usize = 0x200;
    pub const ID: Range<usize> = 8..40;
    pub const MODEL: Range<usize> = 104..108;
    pub const VERSION: Range<usize> = 108..112;
    pub const RING: usize = 118;
}

impl AppImage {
    /// Read what `cargo app-tool build` wrote: the serialized app (`.tapp`), its proof beside it,
    /// and its ring's root packet in the `root-packets` directory of the same artifact tree.
    pub fn from_build(tapp: &Path) -> Result<Self> {
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .with_context(|| format!("could not read {}", path.display()))
        };
        let root_packets = tapp
            .ancestors()
            .nth(3)
            .context("the app is not in an artifact tree")?
            .join("root-packets");
        // Ring 0 has its own root packet; rings 1 and 2 share one. Which ring the app is in is
        // in its header, so try the common one first.
        let app = read(tapp)?;
        let proof = read(&tapp.with_extension("proof"))?;
        let shared = Self::from_parts(
            &app,
            &proof,
            &read(&root_packets.join("rootpacket_ring12.tmr")).unwrap_or_default(),
        );
        match shared {
            Ok(image) if image.header[header::RING] != 0 => Ok(image),
            _ => Self::from_parts(
                &app,
                &proof,
                &read(&root_packets.join("rootpacket_ring0.tmr"))?,
            ),
        }
        .with_context(|| format!("{} is not a Trezor app", tapp.display()))
    }

    /// The same, from the three files' contents: for an app that carries the image inside it.
    pub fn from_parts(tapp: &str, proof: &str, root_packet: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Tapp {
            header: String,
            chunks: Vec<String>,
            chunk_hashes: Vec<String>,
        }
        #[derive(Deserialize)]
        struct Proof {
            proof: Vec<String>,
        }
        #[derive(Deserialize)]
        struct RootPacket {
            root_packet: String,
        }
        let app: Tapp = serde_json::from_str(tapp).context("unreadable app image")?;
        let proof: Proof = serde_json::from_str(proof).context("unreadable app proof")?;
        let root_packet: RootPacket =
            serde_json::from_str(root_packet).context("unreadable root packet")?;
        let header = STANDARD.decode(app.header)?;
        ensure!(
            header.len() == header::LENGTH && header.starts_with(header::MAGIC),
            "not a Trezor app image"
        );
        ensure!(
            app.chunks.len() == app.chunk_hashes.len(),
            "the app's chunks and hashes do not match up"
        );
        let chunks = app
            .chunks
            .iter()
            .zip(&app.chunk_hashes)
            .map(|(chunk, hash)| Ok((STANDARD.decode(chunk)?, hex::decode(hash)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            header,
            chunks,
            proof: hex::decode(proof.proof.concat())?,
            root_packet: STANDARD.decode(root_packet.root_packet)?,
        })
    }

    fn text(&self, range: std::ops::Range<usize>) -> String {
        String::from_utf8_lossy(&self.header[range])
            .trim_end_matches('\0')
            .to_owned()
    }

    /// The app's identifier, e.g. `anzen.app`.
    pub fn id(&self) -> String {
        self.text(header::ID)
    }

    /// The Trezor model the image was built for, e.g. `T3W1`.
    pub fn model(&self) -> String {
        self.text(header::MODEL)
    }

    /// Identifies this exact build.
    fn fingerprint(&self) -> Vec<u8> {
        Sha256::digest(&self.header).to_vec()
    }

    /// When the root packet was signed. A Trezor that already holds this root packet does not
    /// ask for it again.
    fn root_packet_timestamp(&self) -> Result<i64> {
        let bytes = self
            .root_packet
            .get(8..16)
            .context("the root packet is too short")?;
        Ok(i64::from_le_bytes(bytes.try_into().expect("eight bytes")))
    }
}

impl<T: Transport> Trezor<T> {
    /// Load `image` and return the instance id its messages are addressed to. The id belongs to
    /// this session: load again after reconnecting (it is quick when the Trezor still holds the
    /// image).
    pub async fn load_app(&mut self, image: &AppImage) -> Result<u32> {
        let version = &image.header[header::VERSION];
        let request = proto::AppLoad {
            id: Some(image.id()),
            version: Some(proto::Version {
                major: Some(version[0].into()),
                minor: Some(version[1].into()),
                patch: Some(version[2].into()),
                build: Some(version[3].into()),
            }),
            // Debug firmware reloads an image whose build differs from the one it holds, so a
            // rebuilt app replaces the old one. Production firmware goes by the version alone.
            fingerprint: Some(image.fingerprint()),
        };
        let (mut answer, mut body) = self
            .call(kind::APP_LOAD, &proto::encode(&request), QUICK)
            .await?;
        if answer == kind::APP_HEADER_REQUEST {
            (answer, body) = self
                .call(
                    kind::APP_HEADER_ACK,
                    &proto::encode(&proto::AppHeaderAck {
                        header: Some(image.header.clone()),
                        proof: Some(image.proof.clone()),
                        root_packet_timestamp: Some(image.root_packet_timestamp()?),
                    }),
                    QUICK,
                )
                .await
                .context("the Trezor did not accept the Anzen app")?;
            if answer == kind::APP_ROOT_PACKET_REQUEST {
                (answer, body) = self
                    .call(
                        kind::APP_ROOT_PACKET_ACK,
                        &proto::encode(&proto::Bytes {
                            value: Some(image.root_packet.clone()),
                        }),
                        QUICK,
                    )
                    .await
                    .context("the Trezor did not accept the Anzen app's signature")?;
            }
            while answer == kind::APP_DATA_CHUNK_REQUEST {
                let wanted: proto::AppDataChunkRequest = proto::decode(&body)?;
                let (data, hash) = wanted
                    .index
                    .and_then(|index| image.chunks.get(index as usize))
                    .context("the Trezor asked for a part of the app that does not exist")?;
                (answer, body) = self
                    .call(
                        kind::APP_DATA_CHUNK_ACK,
                        &proto::encode(&proto::AppDataChunkAck {
                            data: Some(data.clone()),
                            hash: Some(hash.clone()),
                        }),
                        QUICK,
                    )
                    .await
                    .context("the Trezor stopped loading the Anzen app")?;
            }
        }
        if answer != kind::APP_LOADED {
            bail!("the Trezor did not load the Anzen app (it answered with message {answer})");
        }
        proto::decode::<proto::AppLoaded>(&body)?
            .instance_id
            .context("the Trezor loaded the app but gave no instance id")
    }

    /// Send one message to the loaded app and return its answer: a message id and bytes. The
    /// app's reviews wait for the person.
    pub async fn app_call(
        &mut self,
        instance: u32,
        id: u16,
        data: &[u8],
    ) -> Result<(u16, Vec<u8>)> {
        let (answer, body) = self
            .call(
                kind::APP_MESSAGE,
                &proto::encode(&proto::AppMessage {
                    instance_id: Some(instance),
                    message_id: Some(id.into()),
                    data: Some(data.to_vec()),
                }),
                REVIEW,
            )
            .await?;
        ensure!(
            answer == kind::APP_RESPONSE,
            "the Trezor answered the app's message with message {answer}"
        );
        let response: proto::AppResponse = proto::decode(&body)?;
        let id = response
            .message_id
            .and_then(|id| u16::try_from(id).ok())
            .context("the app's answer has no id")?;
        Ok((id, response.data.unwrap_or_default()))
    }
}
