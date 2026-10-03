//! Inter-frame buffering for the NAL decryptor while the inter-slice encryption is undecided.

use super::{NalDecryptor, annex_b};

const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;
const INTER_SAMPLES: usize = 8;

pub(super) enum PendingNal {
    Ready(Vec<u8>),
    Inter(Vec<u8>, usize),
}

impl NalDecryptor {
    pub(super) fn put(&mut self, data: Vec<u8>) -> Vec<u8> {
        let Some(pending) = &mut self.pending else {
            return data;
        };
        self.pending_bytes = self.pending_bytes.saturating_add(data.len());
        pending.push_back(PendingNal::Ready(data));
        self.guard_pending()
    }

    pub(super) fn put_inter(&mut self, nal: &[u8], header: usize) -> Vec<u8> {
        if self.pending.is_none() {
            return self.inter_bytes(nal, header);
        }
        self.pending_bytes = self.pending_bytes.saturating_add(nal.len());
        if let Some(pending) = &mut self.pending {
            pending.push_back(PendingNal::Inter(nal.to_vec(), header));
        }
        if nal.len() >= header + 16 {
            self.inter_clear.push(nal[header]);
            self.inter_decrypted
                .push(self.decrypt_first(&nal[header..header + 16])[0]);
            if self.inter_clear.len() >= INTER_SAMPLES {
                let clear_unique = unique_count(&self.inter_clear);
                let decrypted_unique = unique_count(&self.inter_decrypted);
                self.inter_encrypted =
                    Some(decrypted_unique * 4 + INTER_SAMPLES < clear_unique * 4);
            }
        }
        if self.inter_encrypted.is_some() {
            self.flush_pending()
        } else {
            self.guard_pending()
        }
    }

    fn guard_pending(&mut self) -> Vec<u8> {
        if self.pending_bytes <= MAX_PENDING_BYTES {
            return Vec::new();
        }
        self.inter_encrypted = Some(false);
        self.flush_pending()
    }

    fn flush_pending(&mut self) -> Vec<u8> {
        let pending = self.pending.take().unwrap_or_default();
        self.pending_bytes = 0;
        let mut output = Vec::new();
        for item in pending {
            match item {
                PendingNal::Ready(data) => output.extend(data),
                PendingNal::Inter(nal, header) => output.extend(self.inter_bytes(&nal, header)),
            }
        }
        output
    }

    fn inter_bytes(&self, nal: &[u8], header: usize) -> Vec<u8> {
        if self.inter_encrypted == Some(true) {
            let mut clear = nal[..header].to_vec();
            clear.extend(self.decrypt_prefix(&nal[header..]));
            annex_b(&clear)
        } else {
            annex_b(nal)
        }
    }
}

fn unique_count(values: &[u8]) -> usize {
    let mut seen = [false; 256];
    for value in values {
        seen[usize::from(*value)] = true;
    }
    seen.into_iter().filter(|value| *value).count()
}
