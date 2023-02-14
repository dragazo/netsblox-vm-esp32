## Installation

You'll need to install the nightly compiler with the extra `rust-src` component.

```sh
rustup toolchain install nightly --component rust-src
```

You'll need the `ldproxy` linker installed, as well as `espflash`.

```sh
cargo install ldproxy
cargo install espflash
```

You'll need to generate an SSL certificate for the internal HTTPS server.

```sh
openssl req -newkey rsa:2048 -nodes -keyout privkey.pem -x509 -days 3650 -out cacert.pem -subj "/CN=NetsBlox VM ESP32-C3"
```
