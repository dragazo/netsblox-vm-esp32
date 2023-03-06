## Installation

To start, you'll need to install the espressif fork of the rust toolchain for building on esp32 targets.
Note that if you are only targetting RISC-V architectures (e.g., esp32-c3) you can instead use the nightly toolchain.

```sh
cargo install espup
espup install
```

You'll need the `ldproxy` linker installed, as well as `espflash`.

```sh
cargo install ldproxy
cargo install espflash
```

You'll need to generate an SSL certificate for the internal HTTPS server.

```sh
openssl req -newkey rsa:2048 -nodes -keyout privkey.pem -x509 -days 3650 -out cacert.pem -subj "/CN=NetsBlox VM ESP32"
```

The current esp tooling is not smart enough to determine the type of connected board.
So next, identify the required target board from the list below.

| board | target |
| ----- | ------ |
| ESP32 | xtensa-esp32-espidf |
| ESP32-S2 | xtensa-esp32s2-espidf |
| ESP32-S3 | xtensa-esp32s3-espidf |
| ESP32-C3 | riscv32imc-esp-espidf |

And finally, build or run the VM for your target.

```sh
cargo +esp run --release --target <target>
```

## Useful Commands

Wipe all contents of flash:

```sh
esptool.py --chip <chip-type> erase_flash
```
