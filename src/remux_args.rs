pub(super) const ARGUMENTS: &[&str] = &[
    "-hide_banner",
    "-loglevel",
    "error",
    "-analyzeduration",
    "500000",
    "-probesize",
    "1000000",
    "-fflags",
    "+genpts+nobuffer",
    "-f",
    "mpeg",
    "-i",
    "pipe:0",
    "-map",
    "0:v:0?",
    "-map",
    "0:a:0?",
    "-c",
    "copy",
    "-bsf:v",
    "dump_extra=freq=keyframe",
    "-muxdelay",
    "0",
    "-flush_packets",
    "1",
    "-mpegts_flags",
    "+resend_headers",
    "-f",
    "mpegts",
    "pipe:1",
];

#[cfg(test)]
mod tests {
    use super::ARGUMENTS;

    #[test]
    fn transport_and_codec_headers_repeat_for_late_subscribers() {
        for required in [
            ["-bsf:v", "dump_extra=freq=keyframe"],
            ["-mpegts_flags", "+resend_headers"],
        ] {
            assert!(
                ARGUMENTS.windows(2).any(|pair| pair == required),
                "missing {required:?}"
            );
        }
    }
}
