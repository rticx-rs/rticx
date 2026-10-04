# rticx-stm32h7

RTICX distribution for the dual-core STM32H7 (Cortex-M7 + Cortex-M4). Each core
runs its own binary; cross-binary software-task spawns travel through a shared
D2 SRAM3 pool with HSEM doorbells, reusing the RTICX task/dispatcher model of
the multi-binary extension (`experiments/multibinary`).

This is the M7 acceptance distribution: the Cortex-M7 initializes the shared
region and releases the Cortex-M4 through `RCC_GCR.BOOT_C2`; both cores map the
shared pool through the MPU and mark themselves ready in the shared state. The
cross-binary transport is bound and generated, and the demo runs an M7↔M4
software-task ping-pong through the generated `cross_spawn` stubs (M7-T2).

## Features

Exactly one core feature must be enabled:

| Feature | Core | Flash bank | RAM | HSEM interrupt |
|---|---|---|---|---|
| `cm7` | Cortex-M7 (global core 0) | `0x0800_0000` | AXI SRAM `0x2400_0000` | `HSEM0` (IRQ 125) |
| `cm4` | Cortex-M4 (global core 1) | `0x0810_0000` | SRAM1 alias `0x1000_0000` | `HSEM1` (IRQ 126) |

The `swtasks` syntax is always available: the distribution binds
`rticx-sw-pass` and `rticx-xbin-pass` in its `#[app]` macro.

## Hardware access

The distribution accesses the HSEM and RCC registers through the `stm32h7`
PAC (`pac::HSEM`, `pac::RCC`), not raw addresses; the device variant is selected
by the same `cm7` / `cm4` feature that selects the core. The
`rticx-stm32h7-bindings` crate carries only the distro-specific IPC pool
geometry (SRAM3 views, pool offsets, budget, HSEM semaphore vocabulary).

The example applications build their consoles on the `stm32-hal2` `Usart`
(driven through `embedded_io::Write`); `console.rs` wraps it in a small
`core::fmt::Write` adapter that `writeln!` can use. `stm32-hal2` only spells
the H747 variants (`h747cm7` / `h747cm4`), so the applications select the
`stm32h747cm7` / `stm32h747cm4` PAC device features. The variant is irrelevant
for the peripherals this demo uses.

## Shared memory

The distribution owns the IPC memory (no `rticx.toml` addresses). It reserves
the 32 KiB D2 SRAM3 block and reports it through the compile-time capability
binding as the `h7-sram3` dual:

| Offset | Contents |
|---|---|
| `+0x0000` | shared ready/epoch state (`rticx_xbin_rt::SharedState`) |
| `+0x0200` | one doorbell word per `(source, target)` direction |
| `+0x1000` | the IPC pool: both directions of the dual, 4096-byte budget |

The Cortex-M7 sees the block at `0x3004_0000` and the Cortex-M4 through the
hardware alias `0x1004_0000`. The MPU maps the whole block Normal,
Non-cacheable, Shareable (region 15 on the M7, region 7 on the M4); Renode does
not model the M7 D-cache, so a green simulation run validates boot, transport
and interrupt routing, never the cache policy.

## Reserved HSEM interrupts

The generated per-pair doorbell routers bind `HSEM0` (M7) / `HSEM1` (M4). The
distribution rejects any user task or dispatcher that tries to bind either
interrupt, so the IPC notification lines can never be stolen by application
code.

## Linker scripts

The distribution's `build.rs` generates the core's `memory.x` into `OUT_DIR`
and adds it to the link search path, so `cortex-m-rt`'s `link.x`
(`INCLUDE memory.x`) picks it up and applications carry no linker script of
their own. The scripts are kept in `linker/memory-cm7.x` and
`linker/memory-cm4.x` and also export the `__rticx_xbin_pool_h7_sram3_start/_end`
symbols checked by `cargo xbin build --verify-elf`.

## Building

```bash
# Build the distribution library for one core.
cargo build -p rticx-stm32h7 --target thumbv7em-none-eabihf --features cm7
cargo build -p rticx-stm32h7 --target thumbv7em-none-eabi  --features cm4

# Build and run the acceptance demo (requires the Renode harness).
cd examples-apps
cargo xbin build --verify-elf
../renode/run.sh \
    target/thumbv7em-none-eabihf/debug/cm7 \
    target/thumbv7em-none-eabi/debug/cm4 4
```

The M7 prints on USART1 and the M4 on USART2:

```text
[M7] boot; initializing shared IPC region and releasing the M4
[M7] shared region ready, RCC_GCR.BOOT_C2 set; waiting for the M4
[M4] released by RCC_GCR.BOOT_C2; mapping shared region and marking ready
[M7] Cortex-M4 marked itself ready; starting the M7 -> M4 ping (M7-T2)
[M4] shared region mapped, HSEM1 receive enabled, ready bit set (M7-T1)
[M4] rx ping seq=1 value=0x00004d37
[M7] rx pong seq=2 value=0x00004d38
[M4] rx ping seq=3 value=0x00004d39
[M7] rx pong seq=4 value=0x00004d3a
[M7] cross-binary ping-pong complete after 4 hops (M7-T2)
```

The ping-pong exercises both directions end to end: the M7 starts it by
calling the generated `PingTask::cross_spawn`, the M4's `PingTask::exec`
answers through the generated `PongTask::cross_spawn`, and the two generated
doorbell routers (HSEM1 on the M4, HSEM0 on the M7) wake the line dispatchers
that run the receiver tasks.

## Limitations

- Async cross-binary tasks are out of scope (the extension's v1 scope).
- Renode models neither the M7 D-cache nor clock gating; the MPU configuration
  is verified statically, not simulated.
- Phase 1 (`cargo xbin sync`) does not type-check the applications: the pass
  terminates the compilation right after writing its manifest, and the typed
  sender stubs are generated and checked at `cargo xbin build` (M7-T2).

## License

MIT
