use std::collections::VecDeque;

use aes::{
    Aes128,
    cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray},
};

use crate::error::BridgeError;

use super::VideoCodec;

#[path = "video_cipher_pending.rs"]
mod pending;
use pending::PendingNal;

const START_CODE: &[u8] = b"\x00\x00\x00\x01";
const ENCRYPTED_PREFIX_BYTES: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EncryptionMode {
    Unknown,
    Clear,
    Encrypted { clear: usize, drop: usize },
}

pub(super) struct NalDecryptor {
    codec: VideoCodec,
    key: [u8; 16],
    mode: EncryptionMode,
    inter_encrypted: Option<bool>,
    inter_clear: Vec<u8>,
    inter_decrypted: Vec<u8>,
    pending: Option<VecDeque<PendingNal>>,
    pending_bytes: usize,
}

impl NalDecryptor {
    pub(super) const fn new(codec: VideoCodec, key: [u8; 16]) -> Self {
        Self {
            codec,
            key,
            mode: EncryptionMode::Unknown,
            inter_encrypted: None,
            inter_clear: Vec::new(),
            inter_decrypted: Vec::new(),
            pending: Some(VecDeque::new()),
            pending_bytes: 0,
        }
    }

    pub(super) fn feed(&mut self, nal: &[u8]) -> Result<Vec<u8>, BridgeError> {
        let header = match self.codec {
            VideoCodec::H264 => 1,
            VideoCodec::Hevc => 2,
        };
        if nal.len() < header {
            return Ok(Vec::new());
        }
        self.classify(nal, header)?;
        if self.mode == EncryptionMode::Unknown {
            return Ok(Vec::new());
        }
        if self.pending.is_some()
            && !matches!(
                self.mode,
                EncryptionMode::Encrypted { clear, drop: 0 } if clear > 0
            )
        {
            self.pending = None;
        }
        self.emit_nal(nal)
    }

    fn classify(&mut self, nal: &[u8], header: usize) -> Result<(), BridgeError> {
        if self.mode != EncryptionMode::Unknown {
            return Ok(());
        }
        let param_set = self.is_param_set(nal[0]);
        if param_set && nal.get(header).is_some_and(|byte| self.body_marker(*byte)) {
            self.mode = EncryptionMode::Clear;
            return Ok(());
        }
        if param_set && nal.len() >= header + 16 {
            let decoded = self.decrypt_first(&nal[header..header + 16]);
            if self.body_marker(decoded[0]) {
                self.mode = EncryptionMode::Encrypted {
                    clear: header,
                    drop: 0,
                };
                return Ok(());
            }
        }
        if nal.len() >= header + 16 {
            let decoded = self.decrypt_first(&nal[header..header + 16]);
            if self.is_param_set(decoded[0])
                && decoded
                    .get(header)
                    .is_some_and(|byte| self.body_marker(*byte))
            {
                self.mode = EncryptionMode::Encrypted {
                    clear: 0,
                    drop: header,
                };
                return Ok(());
            }
        }
        if nal.len() >= 16 {
            let decoded = self.decrypt_first(&nal[..16]);
            if self.is_param_set(decoded[0])
                && decoded
                    .get(header)
                    .is_some_and(|byte| self.body_marker(*byte))
            {
                self.mode = EncryptionMode::Encrypted { clear: 0, drop: 0 };
                return Ok(());
            }
        }
        if param_set && nal.len() >= header + 16 {
            return Err(BridgeError::Authentication);
        }
        Ok(())
    }

    fn emit_nal(&mut self, nal: &[u8]) -> Result<Vec<u8>, BridgeError> {
        match self.mode {
            EncryptionMode::Unknown => Ok(Vec::new()),
            EncryptionMode::Clear => Ok(self.put(annex_b(nal))),
            EncryptionMode::Encrypted { drop, .. } if drop > 0 => Ok(self.emit_hint(nal, drop)),
            EncryptionMode::Encrypted { clear: 0, .. } => {
                let clear = self.decrypt_prefix(nal);
                Ok(self.put(annex_b(&clear)))
            }
            EncryptionMode::Encrypted { clear, drop: 0 } => {
                if nal.len() <= clear {
                    return Ok(self.put(annex_b(nal)));
                }
                if self.encrypted_nal_type(nal[0]) {
                    let mut clear_nal = nal[..clear].to_vec();
                    clear_nal.extend(self.decrypt_prefix(&nal[clear..]));
                    Ok(self.put(annex_b(&clear_nal)))
                } else {
                    Ok(self.put_inter(nal, clear))
                }
            }
            EncryptionMode::Encrypted { .. } => {
                Err(BridgeError::Upstream("invalid encrypted-video mode".into()))
            }
        }
    }

    fn emit_hint(&mut self, nal: &[u8], drop: usize) -> Vec<u8> {
        let decoded = (nal.len() >= drop + 16).then(|| self.decrypt_first(&nal[drop..drop + 16]));
        if decoded
            .as_ref()
            .is_some_and(|value| value[..drop] == nal[..drop])
        {
            let clear = self.decrypt_prefix(&nal[drop..]);
            self.put(annex_b(&clear))
        } else if nal.len() >= drop * 2 && nal[drop..drop * 2] == nal[..drop] {
            self.put(annex_b(&nal[drop..]))
        } else {
            self.put(annex_b(nal))
        }
    }

    fn decrypt_prefix(&self, value: &[u8]) -> Vec<u8> {
        let decrypt_bytes = value.len().min(ENCRYPTED_PREFIX_BYTES) / 16 * 16;
        let mut output = value.to_vec();
        let cipher = Aes128::new(GenericArray::from_slice(&self.key));
        for block in output[..decrypt_bytes].chunks_exact_mut(16) {
            cipher.decrypt_block(GenericArray::from_mut_slice(block));
        }
        output
    }

    fn decrypt_first(&self, value: &[u8]) -> [u8; 16] {
        let mut output = [0_u8; 16];
        output.copy_from_slice(&value[..16]);
        let cipher = Aes128::new(GenericArray::from_slice(&self.key));
        cipher.decrypt_block(GenericArray::from_mut_slice(&mut output));
        output
    }

    const fn body_marker(&self, byte: u8) -> bool {
        match self.codec {
            VideoCodec::H264 => matches!(
                byte,
                44 | 66 | 77 | 83 | 86 | 88 | 100 | 110 | 118 | 122 | 128 | 244
            ),
            VideoCodec::Hevc => byte == 0x0c,
        }
    }

    const fn is_param_set(&self, first: u8) -> bool {
        match self.codec {
            VideoCodec::H264 => first & 0x1f == 7,
            VideoCodec::Hevc => (first >> 1) & 0x3f == 32,
        }
    }

    const fn encrypted_nal_type(&self, first: u8) -> bool {
        match self.codec {
            VideoCodec::H264 => matches!(first & 0x1f, 5 | 7 | 8),
            VideoCodec::Hevc => matches!((first >> 1) & 0x3f, 16..=21 | 32..=34),
        }
    }
}

fn annex_b(nal: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(START_CODE.len() + nal.len());
    output.extend_from_slice(START_CODE);
    output.extend_from_slice(nal);
    output
}
