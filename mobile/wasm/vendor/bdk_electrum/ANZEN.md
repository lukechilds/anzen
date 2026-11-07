# Browser-only compatibility patch

Source: crates.io `bdk_electrum` 0.23.2, upstream commit
`8f281da1bf1cd461d0a33381f962cc4716960727`, `crates/electrum`.
The two Rust source files are unchanged. Used under the included MIT license.

The standalone `mobile/wasm` workspace patches this dependency only. Native Anzen
continues to use crates.io. The manifest makes electrum-client's `proxy` feature
non-WASM-only: upstream enables it unconditionally, but its SOCKS UDP transport
only implements vectored I/O on Unix/Windows. Browser I/O uses a supplied
`Read + Write` WebSocket bridge, not TCP, UDP or SOCKS.

Remove this vendor directory and the patch once upstream makes proxy optional or
target-specific. When upgrading, replace the upstream sources together; do not
change discovery, transaction validation or chain-update logic here.
