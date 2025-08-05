With the `defmt` feature enabled, `yap` can decode incoming defmt packets!

The Defmt Parsing setting controls how incoming bytes will be treated:

- Disabled: The default, all incoming bytes are treated as text.
- Raw: All bytes will be treated as raw, uncompressed defmt frames. Any error in parsing will cease further attempts.
- UnframedRzcobs: Assumes all frames will be rzCOBS-encoded, terminated by `0x00`. Errors during parsing will be handled gracefully.
- FramedRzcobs: Expects defmt frames to be framed as `0xFF 0x00 <CONTENT> 0x00` (The terminating 0x00 is the same terminating byte from rzCOBS encoding). Any bytes outside of those frames will be treated as text. **If you're using an ESP32, you probably want this one!**

And when setting up a espflash .ELF profile, you can add the line `defmt = true` to automatically begin using the ELF to decode any packets sent after flashing!

FramedRzcobs matches the framing scheme used by `esp-rs`'s [`espflash`](https://github.com/esp-rs/espflash) and [`esp-println`](https://github.com/esp-rs/esp-hal/tree/main/esp-println)!
