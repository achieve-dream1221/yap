// desired interface:

// yap
//   - opens port selection menu

// yap COM4
//   - connects to given port at default baud (might need to be in config? 115200 if not set)
// yap COM4 9600
//   - connects to given port at given baud
// - if these fail, then show an error and sys::exit(1)
// - or instead, show an error and drop on port selection screen, but an --option can skip that and just close?
// - both should skip scanning for serial ports and try to directly connect to the given port
// - maybe also allow USB PID+VID as an "address"? would prefer to accept same formats as usb ignore configs

// option to print out all possible AppActions then exit?
