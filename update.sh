#!/usr/bin/env bash

cargo update
cargo check
cd runtime/wasm
cargo update
cd ../..
./build
cargo run