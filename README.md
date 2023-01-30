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
