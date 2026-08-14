use std::collections::VecDeque;

use bytes::Bytes;

const TS_PACKET_BYTES: usize = 188;
const RECENT_PACKET_LIMIT: usize = 512;
const MAX_WARM_BYTES: usize = 8 * 1024 * 1024;
const MAX_INGEST_BYTES: usize = 64 * 1024;

pub(super) struct TsWarmCache {
    pending: Vec<u8>,
    recent: VecDeque<[u8; TS_PACKET_BYTES]>,
    segment: Vec<u8>,
    recent_limit: usize,
    maximum_bytes: usize,
    ready: bool,
}

impl Default for TsWarmCache {
    fn default() -> Self {
        Self::with_limits(RECENT_PACKET_LIMIT, MAX_WARM_BYTES)
    }
}

impl TsWarmCache {
    fn with_limits(recent_limit: usize, maximum_bytes: usize) -> Self {
        Self {
            pending: Vec::with_capacity(TS_PACKET_BYTES * 2),
            recent: VecDeque::with_capacity(recent_limit),
            segment: Vec::new(),
            recent_limit,
            maximum_bytes,
            ready: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.pending.clear();
        self.recent.clear();
        self.segment.clear();
        self.ready = false;
    }

    pub(super) fn ingest(&mut self, bytes: &[u8]) {
        if bytes.len() > MAX_INGEST_BYTES
            || self
                .pending
                .len()
                .checked_add(bytes.len())
                .is_none_or(|total| total > MAX_INGEST_BYTES + TS_PACKET_BYTES)
        {
            self.clear();
            return;
        }
        self.pending.extend_from_slice(bytes);
        self.consume_packets();
    }

    pub(super) fn snapshot(&self) -> Option<Bytes> {
        (self.ready && !self.segment.is_empty()).then(|| Bytes::copy_from_slice(&self.segment))
    }

    fn consume_packets(&mut self) {
        let mut offset = 0;
        while self.pending.len().saturating_sub(offset) >= TS_PACKET_BYTES {
            if self.pending[offset] != 0x47 {
                offset += 1;
                continue;
            }
            if self.pending.len().saturating_sub(offset) >= TS_PACKET_BYTES * 2
                && self.pending[offset + TS_PACKET_BYTES] != 0x47
            {
                offset += 1;
                continue;
            }
            let mut packet = [0_u8; TS_PACKET_BYTES];
            packet.copy_from_slice(&self.pending[offset..offset + TS_PACKET_BYTES]);
            self.consume_packet(packet);
            offset += TS_PACKET_BYTES;
        }
        if offset > 0 {
            self.pending.drain(..offset);
        }
        if self.pending.len() > TS_PACKET_BYTES - 1 {
            let retain_from = self.pending.len() - (TS_PACKET_BYTES - 1);
            self.pending.drain(..retain_from);
        }
    }

    fn consume_packet(&mut self, packet: [u8; TS_PACKET_BYTES]) {
        let random_access = is_random_access(&packet);
        if random_access {
            self.segment.clear();
            for recent in &self.recent {
                self.segment.extend_from_slice(recent);
            }
            self.segment.extend_from_slice(&packet);
            self.ready = self.segment.len() <= self.maximum_bytes;
        } else if self.ready {
            if self
                .segment
                .len()
                .checked_add(TS_PACKET_BYTES)
                .is_some_and(|total| total <= self.maximum_bytes)
            {
                self.segment.extend_from_slice(&packet);
            } else {
                self.segment.clear();
                self.ready = false;
            }
        }

        self.recent.push_back(packet);
        while self.recent.len() > self.recent_limit {
            self.recent.pop_front();
        }
    }
}

fn is_random_access(packet: &[u8; TS_PACKET_BYTES]) -> bool {
    let adaptation_control = (packet[3] >> 4) & 0x03;
    if !matches!(adaptation_control, 2 | 3) {
        return false;
    }
    let adaptation_length = usize::from(packet[4]);
    adaptation_length > 0 && adaptation_length <= TS_PACKET_BYTES - 5 && packet[5] & 0x40 != 0
}

#[cfg(test)]
mod tests {
    use super::{TS_PACKET_BYTES, TsWarmCache};

    fn packet(marker: u8, random_access: bool) -> [u8; TS_PACKET_BYTES] {
        let mut packet = [marker; TS_PACKET_BYTES];
        packet[0] = 0x47;
        packet[1] = 0x01;
        packet[2] = 0x00;
        packet[3] = 0x30;
        packet[4] = 1;
        packet[5] = u8::from(random_access) * 0x40;
        packet
    }

    #[test]
    fn split_and_misaligned_input_yields_a_decodable_warm_snapshot() {
        let mut cache = TsWarmCache::with_limits(4, TS_PACKET_BYTES * 8);
        let bootstrap = packet(1, false);
        let keyframe = packet(2, true);
        let following = packet(3, false);
        let mut bytes = vec![0xaa, 0xbb];
        bytes.extend_from_slice(&bootstrap);
        bytes.extend_from_slice(&keyframe);
        bytes.extend_from_slice(&following);

        cache.ingest(&bytes[..97]);
        assert!(cache.snapshot().is_none());
        cache.ingest(&bytes[97..]);

        let Some(snapshot) = cache.snapshot() else {
            panic!("warm snapshot was not produced");
        };
        assert_eq!(snapshot.len(), TS_PACKET_BYTES * 3);
        assert_eq!(snapshot[0], 0x47);
        assert_eq!(snapshot[TS_PACKET_BYTES + 6], 2);
        assert_eq!(snapshot[TS_PACKET_BYTES * 2 + 6], 3);
    }

    #[test]
    fn a_new_random_access_point_rotates_the_bounded_segment() {
        let mut cache = TsWarmCache::with_limits(1, TS_PACKET_BYTES * 4);
        cache.ingest(&packet(1, true));
        cache.ingest(&packet(2, false));
        cache.ingest(&packet(3, false));
        cache.ingest(&packet(4, true));

        let Some(snapshot) = cache.snapshot() else {
            panic!("rotated snapshot was not produced");
        };
        assert_eq!(snapshot.len(), TS_PACKET_BYTES * 2);
        assert_eq!(snapshot[6], 3);
        assert_eq!(snapshot[TS_PACKET_BYTES + 6], 4);
    }

    #[test]
    fn oversized_gop_stays_unavailable_until_the_next_keyframe() {
        let mut cache = TsWarmCache::with_limits(0, TS_PACKET_BYTES * 2);
        cache.ingest(&packet(1, true));
        cache.ingest(&packet(2, false));
        assert!(cache.snapshot().is_some());
        cache.ingest(&packet(3, false));
        assert!(cache.snapshot().is_none());
        cache.ingest(&packet(4, true));
        assert!(cache.snapshot().is_some());
    }
}
