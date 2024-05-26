# Dfu-flasher

A tool to flash ARM based CPU's such as stm32.

# Examples

## Read

Read from address 0x0800_0000 1024 bytes and save to a some_file.bin.

```dfu-flasher --bus-device BUS:DEVICE read 0x8000_0000:1024 --file-name some_file.bin```

## Write

Write to flash address 0x0800_0000 1024 using some_file.bin as input.

```dfu-flasher --bus-device BUS:DEVICE read 0x8000_0000:1024 --file-name some_file.bin```

