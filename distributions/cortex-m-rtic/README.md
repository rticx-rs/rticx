# `rtic` — Single-core Cortex-M RTIC distribution

This distribution targets single-core Cortex-M microcontrollers, mirroring the
upstream RTIC cortex-m backend. It supports two mutually-exclusive locking
strategies selected at crate-feature time:

| Feature   | Architecture          | Locking mechanism                          |
|-----------|-----------------------|--------------------------------------------|
| *(default)* | armv7-m and above    | BASEPRI register (priority threshold)      |
| `armv6m`  | armv6-m (M0/M0+/M23)  | Interrupt source masking via NVIC ISER/ICER|

Software tasks are enabled by default through the `swtasks` feature; disable it (`--no-default-features`) for a hardware-task-only build.

## Layout

```
cortex-m-rtic/
├── src/                 # `rtic` library: re-exports `app` macro and runtime exports
├── rtic-macro/          # proc-macro crate implementing CorePassBackend + SwPassBackend
└── example-apps/
    ├── armv7m-app/      # stm32f103 (thumbv7m) example using BASEPRI locking
    └── armv6m-app/     # stm32f0xx  (thumbv6m) example using source masking
```

## Building the examples

```bash
# armv7-m / BASEPRI
cd example-apps/armv7m-app && cargo build --example hello_rtic

# armv6-m / source masking
cd example-apps/armv6m-app && cargo build --example hello_rtic
```