# Dfu flasher

Experimental Rust implementation of STM32 DFU flasher heavily based on STM32 dfu-util at https://github.com/dsigma/dfu-util/.

# Dependencies

For full list see Cargo.toml

 - structopt
 - usbapi-rs

# Works

Currently only STM32F205 supported on address range >= 0x0801_0000 < 0x0802_0000

 - [X] Reset STM32 to application mode.
 - [X] Read from STM32 flash
 - [X] Erase/Write to STM32 flash.
 - [X] Mass erase.

# To do

 - Read flash info to calculate allowed pages sizes.

