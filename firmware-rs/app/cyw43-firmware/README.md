# CYW43439 firmware blobs

The Pico 2 W's Wi-Fi silicon has no on-board flash: the host uploads the whole
firmware image over the SPI bus at every boot. These are those bytes. SPEC §7.1
calls for them checked in, and `net::wifi` hands them to `cyw43::new` /
`Control::init` through `cyw43::aligned_bytes!`, which is an `include_bytes!`
into `.rodata` — so they cost flash, never RAM.

| File | Bytes | What it is |
|---|---:|---|
| `43439A0.bin` | 231,077 | The CYW43439 WLAN firmware image |
| `43439A0_clm.bin` | 984 | Country Locale Matrix — the regulatory table |
| `nvram_rp2040.bin` | 742 | Board NVRAM: the RF/antenna calibration for the module Raspberry Pi puts on the Pico W and Pico 2 W |
| `nvram_rp2040.txt` | 739 | The same NVRAM as editable text, kept for readability |

## Provenance

Copied verbatim from [embassy]'s `cyw43-firmware/` at commit
`f51a37a2fb4a9663b67e29086e77f855478ef9e0` (2026-04-21), which in turn tracks
[georgerobotics/cyw43-driver]'s `firmware/` directory — Infineon's own release.

```
5555e0261da2610a500d68c18d895cace0152bbefbf76f4aa683ebce77e3d7eb  43439A0.bin
e712b3d218e8b1e2747b092e03b8b0afcb8c8c8e355d2a4a0d47b493800f3f89  43439A0_clm.bin
4904bdbb0c937bd0ac2eb2a1d62f2da4dd90e32082384e02874e8d671b0f330d  nvram_rp2040.bin
```

Re-verify after any update with `sha256sum` against the upstream files.

## Why vendored rather than a crate

There is a `cyw43-firmware` crate on crates.io (v0.1.0, published by a third
party, not by embassy). Its `43439A0.bin` and `43439A0_clm.bin` are
byte-identical to the two above — checked, not assumed — but it predates
`cyw43` 0.7.0's API change that made the **board NVRAM a required argument** to
`cyw43::new`, and it does not ship an NVRAM blob at all. There is no crate that
carries all three, so the choice was vendored bytes or a crate plus a vendored
blob, and one mechanism beats two.

`nvram_rp2040.bin` is the right board file for the Pico 2 W despite the name:
the Pico W and Pico 2 W carry the same Infineon module, and embassy's own
`examples/rp235x/src/bin/blinky_wifi.rs` passes exactly this file.

## Licence

Infineon Permissive Binary License — `LICENSE-permissive-binary-license-1.0.txt`
in this directory. Redistribution in binary form is permitted; the terms travel
with the bytes, which is why the licence file sits beside them rather than being
folded into the repository's own licensing.

[embassy]: https://github.com/embassy-rs/embassy/tree/main/cyw43-firmware
[georgerobotics/cyw43-driver]: https://github.com/georgerobotics/cyw43-driver/tree/main/firmware
