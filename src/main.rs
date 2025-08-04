fn main() -> color_eyre::Result<()> {
    yap::run()
}

// Sooner TODOs:
// deduplication of code for
// defmt
// logging defmt
// lib.rs startup
// "fuzz" testing with reconsume_raw, just compare cloned structs
// macros feature on user echo
// check defmt hidden line behavior
// test empty rx buffer
// event carousel box-less

// General TODOs:
// Mouse select in line mode?
//   and in Hex view to show make finding bytes easier
// Notification History
// Max buffer size (currently unlimited)

// Far future TODOs:
// Serial Forwarding + Loopback support
// TCP Socket support

// Unimportant but neat TODOs:
// Click on yap bigtext to swap style
// Click on port info in terminal menu to invert style?
