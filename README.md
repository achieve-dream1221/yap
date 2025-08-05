# yap - A friendly serial terminal.

[![Build](https://github.com/nullstalgia/yap/actions/workflows/build.yml/badge.svg)](https://github.com/nullstalgia/yap/actions/workflows/build.yml) [![Release](https://github.com/nullstalgia/yap/actions/workflows/release.yml/badge.svg)](https://github.com/nullstalgia/yap/actions/workflows/release.yml)

<img width="1080" height="607" alt="a split view of yap's port selection screen and port interaction screen" src="https://github.com/user-attachments/assets/fa42e8e8-5481-4600-8c6f-532a4a86d4d9" />

### Features

- User-friendly interface for interacting with Serial/COM ports.
- Optional Pseudo-shell mode to allow preparing an input before sending, with history.
- Intelligent auto-reconnect (checks for devices with matching characteristics, if port path changes unexpectedly).
- Text can be colored by incoming ANSI commands, or by user-created color rules, supporting matching by either regex or string literals.
- Connect to a device from the command line by supplying USB PID+VID
- Log recieved port data to disk as UTF-8 processed text and/or raw bytes.
- Macros with categories to organize commonly sent payloads.
- Support for flashing connected ESP32 devices with .bin/.elf files!
  - Powered by [esp-rs/espflash](https://github.com/esp-rs/espflash)!
- Support for decoding incoming bytes as [defmt](https://github.com/knurling-rs/defmt) frames.
- Configurable keybinds, including for Macros and ESP32 flashing!
  - Keybinds can have several actions that run in order, so you can flash a device and send setup commands if it finishes successfully.
- Hex view to see raw contents of incoming inputs.
- Allow hiding specific devices from Port Selection screen.
- Releases downloaded from GitHub can self-update!
- Cross-Platform!

# Showcase

### Auto-Reconnection Example:
https://github.com/user-attachments/assets/647d0172-3f79-4d47-974d-48f344adc645

### espflash Flashing Example:
https://github.com/user-attachments/assets/d14e6aa2-51d1-489f-b8ba-ca88f74ad3d2

# Pre-compiled Downloads

Pre-compiled portable binaries can be found in [Releases](https://github.com/nullstalgia/yap/releases).

`yap-full` builds have all optional features enabled!

`yap-lite` builds exclude less universally-applicable features (currently excluded: `espflash`, `defmt`, `defmt_watch`).

# Installation

Crates.io releases are not yet available (due to some dependencies not being available/yet released to crates.io).

Local installation can be accomplished by running

```
cargo install --git https://github.com/nullstalgia/yap --no-default-features --features yap-full
```

(or `yap-lite` or any other combination of features).

Avoid enabling the `self-replace` or `portable` features on installed builds!

# Usage

Open port selection:

```
yap
```

Try to connect to port at `COM1`:

```
yap COM1
```

Try to connect to port at `COM1` with 9600 baudrate:

```
yap COM1 9600
```

Try to connect to found USB device with VID 303A and PID 1001:

```
yap 303A:1001
```

Try to connect to found USB device with VID 303A and PID 1001 and Serial Number 123456 with 9600 baudrate:

```
yap 303A:1001:123456 9600
```

## Antivirus notice:

Downloaded Windows builds of `yap` may falsely trigger Windows Defender, likely due to the executable's capacity to self-update.

Release builds have GitHub Attestations enabled to help you to verify their authenticity!


## Acknowledgements:

Inspired by the likes of [minicom](https://salsa.debian.org/minicom-team/minicom)/[picocom](https://github.com/npat-efault/picocom), [Termite](https://www.compuphase.com/software_termite.htm), [CuteCom](https://cutecom.sourceforge.net/), [PuTTY](https://www.chiark.greenend.org.uk/~sgtatham/putty/), and others.

This wouldn't have been possible without the prior work of the original [`ansi-to-tui`](https://github.com/ratatui/ansi-to-tui) crate, [`esp-rs`'s team](https://github.com/esp-rs), [Ferrous Systems](https://ferrous-systems.com/), and many many other incredible crate authors and educators. Thank you all!
