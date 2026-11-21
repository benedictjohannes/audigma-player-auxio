# Third-party licenses

Audigma Player is licensed under the GNU General Public License v3.0 or later (see [LICENSE](LICENSE)). It is a fork of [Auxio](https://github.com/OxygenCobalt/Auxio) (GPL-3.0-or-later, Copyright (c) Auxio Project), based on release v4.1.5. Files taken from Auxio keep Auxio's copyright headers; files we added carry "Audigma Player Project" headers. This file lists what the fork adds or builds on top of Auxio. For Auxio's own dependencies see [Auxio's licenses page](https://github.com/OxygenCobalt/Auxio/wiki/Licenses).

## Audigma (PCM to DSD conversion)

The conversion pipeline comes from [audigma](https://github.com/benedictjohannes/audigma), dual-licensed **MIT OR Apache-2.0**. This app links these crates (git revision recorded in `engine/native/audigma-glue/Cargo.lock`): `audigma-dsp`, `audigma-engine`, `audigma-format`, `audigma-log`, `audigma-settings`. Their dependencies are listed below. The JNI glue (`engine/native/audigma-glue`) is GPL-3.0-or-later.

## Rust crates linked into `libaudigma_glue.so`

Taken from `cargo tree --target aarch64-linux-android` (normal dependencies, proc-macros excluded as they are not linked). Where a crate offers a choice ("A OR B"), the app may be used under either; we choose the permissive option. Full texts are in each crate's repository and in the published crate archive.

| Crate | Version | License | Source |
|---|---|---|---|
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | https://github.com/oyvindln/adler2 |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | https://github.com/bitflags/bitflags |
| bytemuck | 1.25.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/Lokathor/bytemuck |
| byteorder | 1.5.0 | Unlicense OR MIT | https://github.com/BurntSushi/byteorder |
| bytes | 1.12.2 | MIT | https://github.com/tokio-rs/bytes |
| cesu8 | 1.1.0 | Apache-2.0/MIT | https://github.com/emk/cesu8-rs |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/cfg-if |
| combine | 4.6.9 | MIT | https://github.com/Marwes/combine |
| crc32fast | 1.5.5 | MIT OR Apache-2.0 | https://github.com/srijs/rust-crc32fast |
| crossbeam-channel | 0.5.17 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| deranged | 0.5.9 | MIT OR Apache-2.0 | https://github.com/jhpratt/deranged |
| extended | 0.1.0 | MIT | https://github.com/depp/extended-rs |
| flate2 | 1.1.11 | MIT OR Apache-2.0 | https://github.com/rust-lang/flate2-rs |
| hound | 3.5.1 | Apache-2.0 | https://github.com/ruuda/hound |
| id3 | 1.17.4 | MIT | https://codeberg.org/polyfloyd/rust-id3 |
| itoa | 1.0.19 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| jni | 0.21.1 | MIT/Apache-2.0 | https://github.com/jni-rs/jni-rs |
| jni-sys | 0.3.1, 0.4.1 | MIT OR Apache-2.0 | https://github.com/jni-rs/jni-sys |
| lazy_static | 1.5.1 | MIT OR Apache-2.0 | https://github.com/rust-lang-nursery/lazy-static.rs |
| libc | 0.2.191 | MIT OR Apache-2.0 | https://github.com/rust-lang/libc |
| log | 0.4.35 | MIT OR Apache-2.0 | https://github.com/rust-lang/log |
| matchers | 0.2.0 | MIT | https://github.com/hawkw/matchers |
| memchr | 2.8.3 | Unlicense OR MIT | https://github.com/BurntSushi/memchr |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide |
| nu-ansi-term | 0.50.3 | MIT | https://github.com/nushell/nu-ansi-term |
| num-complex | 0.4.6 | MIT OR Apache-2.0 | https://github.com/rust-num/num-complex |
| num-conv | 0.2.2 | MIT OR Apache-2.0 | https://github.com/jhpratt/num-conv |
| num-integer | 0.1.47 | MIT OR Apache-2.0 | https://github.com/rust-num/num-integer |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | https://github.com/rust-num/num-traits |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | https://github.com/matklad/once_cell |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT | https://github.com/taiki-e/pin-project-lite |
| powerfmt | 0.2.1 | MIT OR Apache-2.0 | https://github.com/jhpratt/powerfmt |
| primal-check | 0.3.4 | MIT OR Apache-2.0 | https://github.com/huonw/primal |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | https://github.com/dtolnay/proc-macro2 |
| quote | 1.0.47 | MIT OR Apache-2.0 | https://github.com/dtolnay/quote |
| regex-automata | 0.4.19 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| regex-lite | 0.1.9 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| regex-syntax | 0.8.12 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| rtrb | 0.4.1 | MIT OR Apache-2.0 | https://github.com/mgeier/rtrb |
| rustfft | 6.4.1 | MIT OR Apache-2.0 | https://github.com/ejmahler/RustFFT |
| rusty_flac | 0.5.1 | Apache-2.0 | https://github.com/Remade-With-Rust/rusty_flac |
| serde | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| serde_spanned | 1.1.3 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| sharded-slab | 0.1.7 | MIT | https://github.com/hawkw/sharded-slab |
| simd-adler32 | 0.3.10 | MIT | https://github.com/mcountryman/simd-adler32 |
| smallvec | 1.16.6 | MIT OR Apache-2.0 | https://github.com/smallvec/smallvec |
| strength_reduce | 0.2.4 | MIT OR Apache-2.0 | http://github.com/ejmahler/strength_reduce |
| symlink | 0.1.0 | MIT/Apache-2.0 | https://gitlab.com/chris-morgan/symlink |
| syn | 2.0.119, 3.0.10 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| thiserror | 1.0.69, 2.0.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| thread_local | 1.1.10 | MIT OR Apache-2.0 | https://github.com/Amanieu/thread_local-rs |
| time | 0.3.55 | MIT OR Apache-2.0 | https://github.com/time-rs/time |
| time-core | 0.1.9 | MIT OR Apache-2.0 | https://github.com/time-rs/time |
| toml | 1.2.1 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_datetime | 1.1.3 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_parser | 1.1.5 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| toml_writer | 1.2.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| tracing | 0.1.44 | MIT | https://github.com/tokio-rs/tracing |
| tracing-appender | 0.2.5 | MIT | https://github.com/tokio-rs/tracing |
| tracing-core | 0.1.36 | MIT | https://github.com/tokio-rs/tracing |
| tracing-log | 0.2.0 | MIT | https://github.com/tokio-rs/tracing |
| tracing-serde | 0.2.0 | MIT | https://github.com/tokio-rs/tracing |
| tracing-subscriber | 0.3.23 | MIT | https://github.com/tokio-rs/tracing |
| transpose | 0.2.3 | MIT OR Apache-2.0 | https://github.com/ejmahler/transpose |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | https://github.com/dtolnay/unicode-ident |
| winnow | 1.0.5 | MIT | https://github.com/winnow-rs/winnow |
| zmij | 1.0.23 | MIT | https://github.com/dtolnay/zmij |

### Symphonia (MPL-2.0)

The following crates are licensed under the **Mozilla Public License 2.0**, a file-level copyleft license. We use them unmodified; their source is at <https://github.com/pdeljanov/Symphonia>, and the MPL text is at <https://www.mozilla.org/MPL/2.0/>.

| Crate | Version |
|---|---|
| symphonia | 0.6.1|MPL-2.0|https://github.com/pdeljanov/Symphonia |
| symphonia-bundle-flac | 0.6.1|MPL-2.0|https://github.com/pdeljanov/Symphonia |
| symphonia-codec-pcm | 0.6.1|MPL-2.0|https://github.com/pdeljanov/Symphonia |
| symphonia-common | 0.6.1|MPL-2.0|https://github.com/pdeljanov/Symphonia |
| symphonia-core | 0.6.1|MPL-2.0|https://github.com/pdeljanov/Symphonia |
| symphonia-format-riff | 0.6.1|MPL-2.0|https://github.com/pdeljanov/Symphonia |
| symphonia-metadata | 0.6.1|MPL-2.0|https://github.com/pdeljanov/Symphonia |

## Android and native components added to or kept from Auxio

| Component | License | Notes |
|---|---|---|
| Auxio | GPL-3.0-or-later | Base of this fork. |
| Media3 (Auxio's patched fork, `media/` submodule, <https://github.com/OxygenCobalt/media>) | Apache-2.0 | Unmodified by us. |
| FFmpeg (decoder extension of the Media3 fork) | LGPL-2.1-or-later | Built by `build_ffmpeg.sh` without `--enable-gpl`. |
| TagLib (`musikr/src/main/cpp/taglib`) | LGPL-2.1 or MPL-1.1 | We add DSF and DSDIFF parsing in `taglib_jni.cpp` (Auxio's JNI code). |
| Jetpack Compose (BOM 2025.12.01), Material 3, Foundation, UI | Apache-2.0 | Used by the settings screen in `engine/`. |
| Other AndroidX, Material Components, Hilt/Dagger, Coil, etc. | Apache-2.0 | As in Auxio. |

## Modifications

Changes to Auxio's files: application ID, name and icon; the about screen; a settings entry that opens our DSD screen; DSF and DSDIFF discovery in the TagLib JNI layer and the song UID; extractor and audio-sink wiring. The complete history is in this repository's git log.
