//! Recording quota admission.

/// Whether one more recording fits: finished files plus the maximum size of
/// every recording still being written (and of the new one) stay in quota.
pub(super) const fn admits(spool: u64, active: u64, max: u64, quota: u64) -> bool {
    spool.saturating_add(active.saturating_add(1).saturating_mul(max)) <= quota
}

#[cfg(test)]
mod tests {
    use super::admits;

    #[test]
    fn active_recordings_reserve_their_maximum() {
        let max = 256;
        assert!(admits(0, 0, max, 512));
        assert!(admits(0, 1, max, 512), "one running plus this one fit");
        assert!(
            !admits(0, 2, max, 512),
            "a third concurrent recording would overrun"
        );
        assert!(!admits(300, 0, max, 512), "finished files count too");
    }
}
