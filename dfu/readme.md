# Dfu library

Rust implementation of STM32 DFU flasher heavily based on STM32 dfu-util at https://github.com/dsigma/dfu-util/.

This crates is the core library and used by dfu-flasher binary crate.

# Dependencies

For full list see Cargo.toml

 - usbapi-rs

# Works

 - [X] Reset STM32 to application mode.
 - [X] Read from STM32 flash
 - [X] Erase/Write to STM32 flash.
 - [X] Mass erase.

