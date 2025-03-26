# Hardware

This project has been developed and tested on an ESP32-S3-DevKitC-1-N32R8V, though it should also work identically for an N16 or an N8.
Other ESP32 devices with similar specifications should also work, but may require different settings for building.
If you are having issues with one of these other ESP32 devices, feel free to open an issue and I can look into adding additional platform support.

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
So you must manually identify the required target from the list below.

| board | target |
| ----- | ------ |
| ESP32 | xtensa-esp32-espidf |
| ESP32-S2 | xtensa-esp32s2-espidf |
| ESP32-S3 | xtensa-esp32s3-espidf |
| ESP32-C3 | riscv32imc-esp-espidf |

```sh
cargo +esp run --release --target <target>
```

## Flash a Pre-Built Image

To flash a pre-built image, first download the image for your appropriate platform.
If you are using an ESP32 device which is not listed, feel free to open an issue, but in the meantime you must instead build from source (see above).

| board | image |
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

To interact with the device, locate the puzzle-shaped icon near the top right of the editor, and select the "Native" extension's "Open Terminal" option.
You can then build any program you desire in the regular code editor and use the terminal window to interact with the device (e.g., upload the program and click the green flag button in the terminal to start execution on the device).

# Peripherals

Peripherals (e.g., LEDs, motors, sensors, etc.) can be added to the device via the configuration page (see setup).
These are specified in JSON, the schema for which can be found [here](peripherals.md).

# Useful Commands

The following command can be used to wipe all contents of the flash storage.
This can be helpful in rare cases where an invalid configuration is sent to the device.
If this resolves your problem, please also open an issue describing what caused the problem.

```sh
pip install esptool
esptool.py --chip <chip-type> erase_flash
```
