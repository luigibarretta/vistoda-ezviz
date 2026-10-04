# EZVIZ VTM research log

Reviewed 2026-09-10 against immutable commit IDs.

| Project | Commit | License | Relevant evidence | Adoption |
| --- | --- | --- | --- | --- |
| [Bobsilvio/ezviz_hp7](https://github.com/Bobsilvio/ezviz_hp7) | `a6c038a3bb8ef2395823c3217dd51d132b4da88f` | MIT, with Apache-2.0 vendored directory | VTM cloud relay, retry lockout, bounded queues, watchdog, GOP cache, AAC/PES quirk | Lifecycle safeguards; no copied code |
| [RenierM26/pyEzvizApi](https://github.com/RenierM26/pyEzvizApi) | `c713642fd99c3467efe1285dfc5d085714a00b50` | Apache-2.0 | VTM/VTDU transport, session reuse, snapshots, MPEG-PS and copy-remux | Compatibility oracle; not linked at runtime |
| [albrzmr/ezviz_hp7](https://github.com/albrzmr/ezviz_hp7) | `b3dcd6e4e7bdb3467f8a164fbb5867a2c672475f` | MIT | Independent CPD7 LAN path, single upstream, warm/keyframe buffer, failure diagnostics | Design cross-check only |
| [LethalEthan/LE-EZVIZ-VS](https://github.com/LethalEthan/LE-EZVIZ-VS) | `35a267cf2523034cd4022224a3dec2b7a0f6dbb7` | LGPL-2.1 | VTM/VTDU handoff, protobuf messages, keepalive and MPEG-PS/RTP variants; encryption incomplete | Protocol corroboration only |
| [Bahrombekk/cloud-cam-viewer](https://github.com/Bahrombekk/cloud-cam-viewer) | `6ff4ad280dc18061cf073fcd9eb7931ab28de34a` | MIT | VTM pagination and encrypted H.264/HEVC RTP behavior | Bounded Rust adaptations; attribution and full license in `third_party/cloud-cam-viewer`; no Python runtime or permanent camera process |

The installed CP4 is a different model from HP7/CP7. Therefore its captured
stream is authoritative: repository observations become requirements only
after an offline fixture or owner-run live canary reproduces them. In
particular, the HP7/CP7 AAC track reportedly appears under an MPEG audio PES
identifier; the bridge does not add audio transcoding unless FFprobe/FFmpeg
prove the CP4 needs it. Video is never re-encoded.

## Official Open Platform SDK boundary

Reviewed 2026-09-09 against the current official Android sample and Maven
dependency `io.github.ezviz-open:ezviz-sdk:5.27.3`.

- `EZPlayer.startVoiceTalk(isDeviceTalkBack)` and `stopVoiceTalk()` expose the
  official talk lifecycle; the sample separately models full duplex and
  press-to-talk microphone state.
- `searchRecordFileFromDevice(...)` returns device recording metadata and
  `startPlayback(EZDeviceRecordInfo)` plays selected SD-card recordings.
- The developer portal also advertises preview, two-way audio and playback;
  its Windows C++ SDK advertises streaming download.
- The exact Maven AAR has SHA-256
  `8c66526597f728a139148a9f997dd56f08d7937e7b38a82be117f1921293abf2` and
  contains only Android `armeabi-v7a`/`arm64-v8a` native libraries, including
  `libHCVoiceTalk.so` and `libEZAudioSDK.so`; it has no Linux ABI.
- The public `EZPlayer`/`EZTalkback` surface can start/stop talk, mute the
  remote side and open/close the Android microphone, but exposes no PCM/audio
  injection callback. Therefore a remote HA browser cannot feed its microphone
  into an Android sidecar through the documented API.

These calls prove product feasibility, not compatibility with this bridge. They
require an EZVIZ Open Platform app/key and a supported native client runtime.
The current provider instead implements the CP4 consumer-account VTM/VTDU path
in a HAOS x86_64 Rust app. Vistoda will not embed Android binaries, acquire new
credential scopes or call destructive storage methods implicitly. A server-side
integration remains gated on approved credentials plus a supported SDK/runtime,
or on a separately proven consumer protocol and read-only CP4 live canary. An
Android bridge is viable only when the SDK and the user's microphone share the
same Android process (for example a dedicated Vistoda Android client); placing
the AAR in a remote Android VM would use that VM's microphone, not the phone's.

Primary sources:

- EZVIZ SDK portal: <https://iusopen.ezviz.com/developer>
- Official Android sample: <https://github.com/Ezviz-Open/EzvizSDK-Android>

## Consumer app settings surface (7.6.1.0824)

Reviewed 2026-09-14 from the owner-provided CP4 settings screenshots, the
published package metadata for `com.ezviz` 7.6.1.0824, and the existing
immutable pyEzvizApi oracle at commit
`c713642fd99c3467efe1285dfc5d085714a00b50`. APKMirror reports the bundle as
vendor-signed (`CN=hikvision`, certificate SHA-256
`45e984f72060dc783490c3905c7efae87397e79fcb033c4e0c54c7acc061e2c5`) but also
states that the developer requested removal of the downloadable artifact. No
EZVIZ APK was present on the development host and no owner handset was
available through ADB, so this pass does not claim a fresh decompilation.

The CP4 UI groups Battery, Intelligent Detection, Message Notification, Audio,
Image, Light and Record List controls, followed by Privacy, Network, Device
Information, sharing and EZVIZ Cloud. The APK-derived compatibility oracle
corroborates typed command families for camera/image parameters, compression,
audio input/output volume, Wi-Fi status/configuration and EZVIZ access, plus
consumer API calls for switch features, alarm light and defence schedules.
Those command IDs prove discovery targets, not CP4-safe write semantics.

Vistoda Home Assistant 0.32.0 therefore exposes the current battery and groups
only the exactly bound official EZVIZ entities under those headings. Writable
rows delegate to Home Assistant's native entity dialog. Missing settings are
shown as unavailable instead of sending guessed private commands. A future
provider-native settings endpoint requires a read/current-value call, typed
validation, same-value canary and read-after-write rollback for each CP4
capability before its control can appear.

## Unified alarm messages (7.6.1.0824)

Reviewed 2026-10-04 by static decompilation of the vendor-signed `com.ezviz`
7.6.1.0824 package (certificate prefix `45:E9:84:F7`). `MessageApi` declares
`GET /v3/unifiedmsg/summarybydevice/v2?stype=` (`summaries[]` with
`deviceSerial`, `total`, `unread`, `topMessage`) and `GET
/v3/unifiedmsg/list/v2?serials=&stype=&limit=20&date=yyyyMMdd&endTime=`
(`hasNext`, `message[]`). `DecryptFileOpener` documents the checksum-key
picture layouts used by ADR-0018. The `endTime` value (last item's `time`;
pyEzvizApi suggests a message ID) and `picCrypt=2` remain unconfirmed by an
owned sample; Vistoda stops on a page without progress and fails closed.

## Video key, microSD and playback (7.6.1.0824)

Reviewed 2026-10-04 by static analysis of the same vendor-signed package
(`com.ezviz.apk` SHA-256
`554fd3260fd32a45e34300c29fc6577bbd7d582751628447e53d0277c123d08c`,
`config.arm64_v8a.apk` SHA-256
`5c01eb60839a983b4ef92e46332dd035fc5457f2f3bd70cec338dbc6f96f599a`), jadx
output of `classes2/6/13/15/22/25.dex` and strings of `libezstreamclient.so`. pyEzvizApi `c713642f` is corroboration only.

**Verification code.** `GetDeviceEncryptKeyTask` feeds the "device
verification code" screen (`DeviceVerifyCodeActivity`) from `VideoGoNetSDK.G`,
which posts `serial`, `checkcode` and `msgType` to
`/api/device/query/encryptkey` and returns `encryptkey` (error 120002 asks for
an SMS code). `MessageDecryptManager` accepts a typed code only when
`MD5Util.getTwiceMD5String(code)` equals `STATUS.encryptPwd`
(case-insensitive) and then stores it as the device password for decryption.
`STATUS.isEncrypt` (0/1) is the image/video encryption switch. The cloud
`encryptkey` is therefore the verification code that `video_cipher.rs` already
derives its AES-128 key from (first 16 bytes, zero padded), exactly like
snapshots and `picCrypt=1` alarm pictures. The player additionally declares
`GET v3/devices/{serial}/{channel}/encryptkey` (`EncryptKeyResp.encryptKey`);
Vistoda keeps the proven legacy endpoint. Toggling uses
`PUT /v3/devices/encryptedInfo/risk` or `/api/device/updateEncrypt` with
`validateCode`/SMS risk control; it is not implemented.

**Storage status.** `StorageActivity` calls `VideoGoNetSDK.H(serial)`:
`POST /api/device/queryStorageStatus` with form `subSerial`. The response is
`resultCode` plus `storageStatus{result, formatingRate, storageList[]}`; each
`StorageInfo` has `index, type, status, capacity, firstRecordTime, hdStatus,
healthLevel`. `StorageAdapter` maps `status` 0 normal (capacity, MiB:
`< 102400` is shown as `capacity/1024` GB), 1 abnormal, 2 unformatted,
3 formatting; an empty list shows "no SD card". No free-space field exists.
The page list `STATUS` also carries `diskNum`, `diskState` and
`optionals.diskCapacity`/`diskHealth`, used by the app only as hints.

**Record index.** `YsPlaybackBusPresenter` chooses by
`isSupportNewSearchRecords()` (`supportExt["256"]`, "record search v3"):
value 2 calls `GET v3/streaming/common/records?deviceSerial&channelNo&
channelSerial&startTime&stopTime&recordType=-1&size=1500&version=1`; value 1
calls `GET v3/streaming/v2/records?deviceSerial&channelNo&startTime&stopTime&
size=100&sortBy=0&requireLabel=0`; otherwise the legacy `v3/streaming/records`
task runs. Request times are camera-local `yyyy-MM-ddT00:00:00` and
`…T23:59:59`. `RecordDataRemoteEzviz` base64-decodes and zlib-inflates
`records` (V2: JSON `[{B, E, Type, Res, Res2}]`, times `yyyy-MM-dd'T'HH:mm:ss`,
`Type == 1` is an event) or `data` (common: 8-byte entries
`[start h, m, s, stop h, m, s, recordType, pad]` added to `baseDay`, at most
`searchCount`; `recordType` 1 event, 8–10 special, others continuous).
`isFinished = 0` marks a truncated V2 day; the continuation cursor (last `E`)
is inferred, not observed. Camera-local times are converted with
`STATUS.optionals.timeZone` (for example `UTC+02:00`).

**Playback spike.** SD playback is the same cloud VTM/VTDU transport but a
different request. `PlayCore.convertFileList` builds `VideoStreamInfo{beginTime,
endTime, seqId}` with `CasUtils.convertCasTime` (`yyyyMMdd'T'HHmmss'Z'` in the
phone's zone despite the `Z`) and clamps the end to `T235959Z` of the start
day; `EZMediaPlayer.startPlayback(List<VideoStreamInfo>)` hands it to native
`CloudClient::startPlayback`. `libezstreamclient.so` contains, beside
`ysproto://` and `/live?`, the fragments `/playback?`, `/download?`, `&chn=`,
`&stream=`, `&seg=`, `&begin=`, `&end=`, `&serial=`, `&streamtag=`, `&ssn=`,
`&rctype=`, `&finterval=`, `&e2ee=` and the messages "receive seekuuid" and
"reach the end of playback. vtmkey=%s". The exact parameter set and order, the
`seg` syntax, whether StreamInfo `0x13b` is reused, and the seek/continue/end
control messages are unproven, so no playback code ships (ADR-0020).
