# Hardware

The code included in this repository should be able to build and run on any esp32 board.
However, there are some specific settings in [`sdconfig.defaults`](sdkconfig.defaults) and [`config.toml`](.cargo/config.toml) that would likely need to be changed.
The included settings are configured to run on an ESP32-S3 N32R8, though it should also work without modification for an N16R8 or an N8R8.

Once you have a working device, check out the [user guide](user-guide.md).

# Installation

To start, you will need to [install Rust](https://www.rust-lang.org/tools/install).

After this, install the `espflash` flasher:

```sh
cargo install espflash
```

From here, you have two options: build from source, or flash a pre-built image

## Build from Source

To build from source, you will need to install the `ldproxy` linker and the espressif fork of the Rust toolchain:

```sh
cargo install ldproxy
cargo install espup
espup install
```

After running the last command, it will likely instruct you to source a shell script in your current terminal session.
It is advised to also put this command into your bashrc file to ensure it is automatically sourced in any future terminal sessions.

Next, generate an SSL certificate for the internal HTTPS server used on the device.

```sh
openssl req -newkey rsa:2048 -nodes -keyout privkey.pem -x509 -days 3650 -out cacert.pem -subj "/CN=NetsBlox VM ESP32"
```

Finally, build and flash the device.
The current esp tooling is not smart enough to determine the type of connected board.
So you must manually identify the required target board from the list below.

| board | target |
| ----- | ------ |
| ESP32 | xtensa-esp32-espidf |
| ESP32-S2 | xtensa-esp32s2-espidf |
| ESP32-S3 | xtensa-esp32s3-espidf |
| ESP32-C3 | riscv32imc-esp-espidf |

```sh
cargo +esp run --release --target <target>
```

## Flash a Pre-Built Binary

First, download the binary for your appropriate platform:

| board | binary |
| ----- | ------ |
| ESP32-S3 | [download](https://dragazo.github.io/netsblox-vm-esp32/esp32s3/img) |

Next, flash the device with the downloaded image:

```sh
espflash flash --flash-size 8mb --partition-table partitions.csv --monitor <IMAGE>
```

# Setup

After you have successfully flashed your device, power cycle it and wait for it to boot.
After booting, you should find a new wireless network called `nb-esp32`.
This is the access point hosted by the device, which you can connect to with password `netsblox`.
After connecting to the network, you can load the device's configuration page at `192.168.71.1`.
Upon loading this page, you can enter valid credentials for your "real" Wi-Fi network (and change the access point SSID/password if desired).

Once credentials are set for the "real" Wi-Fi network, power cycle the board and reload the page once booted.
You should now see a local IP listed for the Wi-Fi client - copy this for later.
Switch back to your regular network with internet access and navigate to the IP you just copied.
This will bring you back to the same web page, but now with internet access.
The link at the top of the page can be used to open the NetsBlox editor with a pre-loaded extension for communicating with the esp32 device (it is advised to use Chrome).

# Peripherals

Peripherals (LEDs, motors, sensors, etc.) can be added to the device via the configuration page (see setup).
These are specified in JSON, the schema for which can be found [here](peripherals.md).

# Useful Commands

Wipe all contents of flash:

```sh
pip install esptool
esptool.py --chip <chip-type> erase_flash
```
