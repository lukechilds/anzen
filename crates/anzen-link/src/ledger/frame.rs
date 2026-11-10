//! Ledger's Bluetooth framing: an APDU travels in frames no longer than the link's MTU. Every
//! frame starts with a tag and a big-endian frame index; the first also carries the APDU's length.

use anyhow::{Result, bail, ensure};

const TAG_APDU: u8 = 0x05;
/// Asks the device for the frame size it accepts. It answers with the same tag.
pub const TAG_MTU: u8 = 0x08;
pub const MTU_REQUEST: [u8; 5] = [TAG_MTU, 0, 0, 0, 0];
/// The frame size every BLE link supports, before the device reports a larger one.
pub const DEFAULT_MTU: usize = 20;

/// The frame size in the device's answer to [`MTU_REQUEST`].
pub fn mtu_answer(frame: &[u8]) -> Option<usize> {
    (frame.len() >= 6 && frame[0] == TAG_MTU).then(|| usize::from(frame[5]))
}

/// Split an APDU into frames of at most `mtu` bytes.
pub fn frames(apdu: &[u8], mtu: usize) -> Result<Vec<Vec<u8>>> {
    ensure!(mtu > 5, "the link's frames are too small to carry an APDU");
    let length =
        u16::try_from(apdu.len()).map_err(|_| anyhow::anyhow!("APDU exceeds 65535 bytes"))?;
    let mut frames = Vec::new();
    let mut rest = apdu;
    loop {
        let index = u16::try_from(frames.len())?;
        let mut frame = vec![TAG_APDU];
        frame.extend_from_slice(&index.to_be_bytes());
        if index == 0 {
            frame.extend_from_slice(&length.to_be_bytes());
        }
        let take = rest.len().min(mtu - frame.len());
        frame.extend_from_slice(&rest[..take]);
        rest = &rest[take..];
        frames.push(frame);
        if rest.is_empty() {
            return Ok(frames);
        }
    }
}

/// Reassembles the frames of one answer.
#[derive(Default)]
pub struct Reassembly {
    expected: Option<usize>,
    next_index: u16,
    data: Vec<u8>,
}

impl Reassembly {
    /// Add a frame. Returns the whole answer once its last frame arrives.
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>> {
        ensure!(frame.len() >= 3, "a Ledger frame is too short");
        ensure!(
            frame[0] == TAG_APDU,
            "unexpected Ledger frame tag {:#04x}",
            frame[0]
        );
        let index = u16::from_be_bytes([frame[1], frame[2]]);
        if index != self.next_index {
            bail!(
                "Ledger frame {index} arrived when {} was expected",
                self.next_index
            );
        }
        self.next_index += 1;
        let body = if index == 0 {
            ensure!(frame.len() >= 5, "the Ledger's first frame has no length");
            self.expected = Some(usize::from(u16::from_be_bytes([frame[3], frame[4]])));
            &frame[5..]
        } else {
            &frame[3..]
        };
        self.data.extend_from_slice(body);
        let expected = self.expected.expect("set by the first frame");
        ensure!(
            self.data.len() <= expected,
            "the Ledger sent more than it announced"
        );
        Ok((self.data.len() == expected).then(|| std::mem::take(&mut self.data)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_apdu_survives_framing_at_every_frame_size() {
        for mtu in [6, 20, 23, 153, 512] {
            for length in [0, 1, mtu - 5, mtu - 4, 3 * mtu, 260] {
                let apdu = (0..length).map(|byte| byte as u8).collect::<Vec<_>>();
                let frames = frames(&apdu, mtu).unwrap();
                assert!(frames.iter().all(|frame| frame.len() <= mtu));
                let mut reassembly = Reassembly::default();
                let mut answer = None;
                for frame in &frames {
                    assert!(answer.is_none(), "frames after the last");
                    answer = reassembly.push(frame).unwrap();
                }
                assert_eq!(answer.unwrap(), apdu, "mtu {mtu}, length {length}");
            }
        }
    }

    #[test]
    fn the_first_frame_carries_the_length_and_later_frames_only_their_index() {
        let frames = frames(&[0xaa; 30], 20).unwrap();
        assert_eq!(frames[0][..5], [0x05, 0, 0, 0, 30]);
        assert_eq!(frames[1][..3], [0x05, 0, 1]);
        assert_eq!((frames[0].len(), frames[1].len()), (20, 18));
    }

    #[test]
    fn frames_out_of_order_or_too_long_are_refused() {
        let frames = frames(&[1; 40], 20).unwrap();
        let mut reassembly = Reassembly::default();
        reassembly.push(&frames[0]).unwrap();
        assert!(reassembly.push(&frames[2]).is_err());
        let mut reassembly = Reassembly::default();
        assert!(reassembly.push(&[0x05, 0, 0, 0, 1, 9, 9]).is_err());
        assert!(Reassembly::default().push(&[0x06, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn the_device_reports_its_frame_size() {
        assert_eq!(mtu_answer(&[0x08, 0, 0, 0, 1, 153]), Some(153));
        assert_eq!(mtu_answer(&[0x05, 0, 0, 0, 1, 153]), None);
    }
}
