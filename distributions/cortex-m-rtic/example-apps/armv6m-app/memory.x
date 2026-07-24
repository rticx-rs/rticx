/* Same app compiled for armv6-m (Cortex-M0/M0+). The locking strategy is
 * selected by the `armv6m` feature on the `rtic` crate, not by memory layout. */
/* STM32F030 memory layout (Cortex-M0 / armv6-m) */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 64K
  RAM   : ORIGIN = 0x20000000, LENGTH = 8K
}