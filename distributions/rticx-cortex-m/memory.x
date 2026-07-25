# Placeholder memory.x
#
# This file is brought in by examples (see `example-apps/armv7m-app/memory.x` and
# `example-apps/armv6m-app/memory.x`) which target concrete Cortex-M devices.
# cortex-m-rt picks up the `memory.x` file residing next to the consuming binary crate.
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 128K
  RAM   : ORIGIN = 0x20000000, LENGTH = 20K
}