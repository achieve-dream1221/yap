fn main() -> color_eyre::Result<()> {
    yap::run()
}

// Sooner TODOs:
// deduplication of code for defmt and logging defmt
// SignPath for release binaries
// ARM builds in releases

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
