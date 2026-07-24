/* Memory layout matching QEMU's `lm3s6965evb` (Stellaris LM3S6965, Cortex-M3).
 * We use this same layout for both the armv7m-app (thumbv7m, BASEPRI) and the
 * armv6m-app (thumbv6m, source-masking) examples; QEMU runs thumbv6m code on
 * the same Cortex-M3 machine since ARMv6-M is a subset of ARMv7-M. */
MEMORY
{
  FLASH : ORIGIN = 0x00000000, LENGTH = 256K
  RAM   : ORIGIN = 0x20000000, LENGTH = 64K
}