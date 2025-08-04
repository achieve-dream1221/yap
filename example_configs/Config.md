Most options are configurable in-app, but some settings reside solely in the `yap.toml`.

```toml
[misc]
log_level = "Trace" ## Max Tracing log level to print
log_tcp_socket = "127.0.0.1:7331" ## Send Tracing log events as text over TCP to this socket

[espflash]
skip_erase_confirm = false ## Skip needing to press Enter Twice when selecting Erase Flash.

[updates]
allow_pre_releases = false ## Also checking for new pre-releases when checking for updates.

[ignored_devices]
show_ttys_ports = false ## Unix only: Show the virtual console ports (/dev/ttyS*)
usb = ["28DE:2102"] ## Ignore ports by USB VID:PID or VID:PID:Serial
# Default set of ignored devices:
# (You can manually remove them from the filter yourself,
# but they will be placed back in if the field ever needs to be regenerated from defaults.)
#
# - Valve Index/Bigscreen Beyond's Bluetooth COM Port (Watchman)
#    VID: 28DE, PID: 2102
name = [] ## Ignore ports by name/path
```
