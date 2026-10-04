# Renode support for the STM32H7 acceptance demo (M7)

Dual-core STM32H7 (Cortex-M7 + Cortex-M4) Renode platform used as the
acceptance target of the multi-binary extension (M7-T1/M7-T2:
`rticx-stm32h7` cross-binary spawns, M7→M4 and M4→M7).

## Provenance

Copied on 2026-09-27 from the standalone simulation repository
`/home/zakaria/stm32-renode` (commit `c7caf8c`), which targets the installed
Renode v1.16.1 and documents the platform in depth:

| File | Origin |
|---|---|
| `platforms/cpus/stm32h7_dualcore.repl` | `platforms/cpus/stm32h7_dualcore.repl` |
| `scripts/single-node/stm32h7_dualcore.resc` | `scripts/single-node/stm32h7_dualcore.resc` |

Only the minimum needed to *run* firmware was copied. The upstream repository
additionally keeps the canonical Peripheral-Script sources
(`scripts/pydev/*.py`), `tools/sync_pydev_into_repl.py` (which regenerates the
embedded copies), the Python unit tests and the Robot integration suite. The
`.repl` copied here is **self-contained**: the RCC/PWR/HSEM/EXTI models are
embedded between its `// >>> pydev:` / `// <<< pydev` markers, so no other file
is required at runtime.

## Usage

```bash
./run.sh <path/to/cm7.elf> <path/to/cm4.elf> [seconds]
```

The script runs Renode headless (`--console --disable-gui`), loads the M7
image on `cpu0`, the M4 image on `cpu1`, attaches `showAnalyzer` to USART1
(M7 console) and USART2 (M4 console), starts the machine, sleeps and quits.
Renode needs the explicit `showAnalyzer` calls even headless: it is what
routes UART output to the log.

Renode does **not** need to be rebuilt; any v1.16.x install works.

## Platform facts relevant to the RTICX H7 distribution

* **Cores.** CPU0 = `cortex-m7` boots from flash bank 1 (`0x0800_0000`);
  CPU1 = `cortex-m4` starts halted with `VTOR = 0x0810_0000` and is released
  only when the M7 writes `RCC_GCR.BOOT_C2`. This is the boot handshake the
  distribution's `post_init` boot release relies on.
* **Execution.** `Machine SetSerialExecution True` keeps both cores
  deterministic (also used for race-free IPC tests).
* **Shared memory.** AXI SRAM (`0x2400_0000`), SRAM1–3 (`0x3000_0000`,
  `0x3002_0000`, `0x3004_0000`, each also aliased at `0x1000_0000`+ for the M4
  view) and SRAM4 (`0x3800_0000`) are shared and coherent. `dtcm`/`itcm` are
  M7-private. The ping-pong demo places its mailbox in SRAM4.
* **Doorbells.** Two independent mechanisms:
  - **HSEM** (`0x5802_6400`): releasing semaphore `n` raises IRQ 125
    (`HSEM1`) on `nvic0` (M7) and IRQ 126 (`HSEM2`) on `nvic1` (M4); the
    release also sets the corresponding status bit on both cores.
  - **EXTI** (`0x5800_0000`): a write to shared `SWIERx` sets the pending bit
    in both `C1PRx` and `C2PRx`; each core sees the line only through its own
    `CxIMR`, and clears it through its own `CxPRx`. Per-core masking makes
    EXTI lines usable as one-directional doorbells.
* **NVICs.** Both cores have their own NVIC at `0xE000_E000` (private bus
  view); peripherals in the base platform drive `nvic0` only, while the HSEM
  and EXTI models drive `nvic0`/`nvic1` programmatically.
* **No cache model.** Renode does not emulate the M7 D-cache, so cache
  maintenance is a no-op here; the non-cacheable MPU configuration required on
  silicon is invisible to the simulation. Do not treat a passing Renode run as
  cache-policy evidence.

## Known limitations (from upstream)

* EXTI is a software-trigger model: GPIO/peripheral lines are not routed
  through it.
* No IPCC on the STM32H7 line; HSEM + EXTI (and a shared doorbell word) are
  the notification paths.
* Clock gating is not modeled; enabling a peripheral clock has no effect.
* Renode's `UART.STM32F7_USART` model treats `USART_CR1.FIFOEN` as reserved
  (logs an unhandled write, harmless) and requires `CR1.UE | CR1.TE` before
  accepting `TDR` writes.
