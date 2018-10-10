aw-server-rust
==============

A reimplementation of aw-server in Rust

Primary features missing:
- Proper HTTP responses on invalid requests
- SQLite performance improvements
- query2 support

Caveats:
- Generally hard to read code at many places
- Poisoned mutex in datastore might still be an issue?
