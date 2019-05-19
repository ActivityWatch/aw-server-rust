aw-server-rust
==============

[![Dependency Status](https://deps.rs/repo/github/activitywatch/aw-server-rust/status.svg)](https://deps.rs/repo/github/activitywatch/aw-server-rust)
[![Build Status](https://travis-ci.org/ActivityWatch/aw-server-rust.svg?branch=master)](https://travis-ci.org/ActivityWatch/aw-server-rust)
[![Coverage Status](https://codecov.io/gh/ActivityWatch/aw-server-rust/branch/master/graph/badge.svg)](https://codecov.io/gh/ActivityWatch/aw-server-rust)

A reimplementation of aw-server in Rust

Primary features missing:
- None?

Caveats:
- Lots of TODO and FIXME comments

Bugs:
- Memory leak during Bucket import? (wtf rust?)

Features missing compared to aw-server python:
- Swagger support

### How to compile

Install rust nightly with rustup

``` rustup default nightly ```

Run cargo build to build

``` cargo build --release ```

### How to run

After compilation you will have an executable at target/release/aw-server-rust

``` ./target/release/aw-server-rust ```

If you want to quick-compile for debugging, run cargo run from the project root

*NOTE:* this will start aw-server-rust on the testing port 5666 instead of port 5600

``` cargo run ```
