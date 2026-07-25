# RTIC Distribution Porting Template

This directory is a **copy-paste starting point** for creating a new RTIC
distribution.  It does not target any specific hardware.

A distribution is the bridge between the RTIC macro framework and your
target MCU.  It tells RTIC how to:
* lock shared resources (SRP ceiling protocol)
* pend interrupts for software tasks
* initialize the interrupt controller
* save/restore state around task execution
* and more ! (depending on the add-on compilation passes you choose to integrate)

## When to use

You're fed-up of a distribution maintainers not fixing the issues you're posting and you decide to take matters in your own hands !
Or other not so important reasons like:
* The existing distributions (`cortex-m-rtic` and friends) do not match your MCU architecture.
* You want to support a different interrupt controller (e.g. NVIC, CLIC, PLIC, custom PLIC).
* You need a different locking strategy than what the existing distributions provide.

## Guides:
Full user and distributor guides are available in the [project wiki](https://github.com/zakimadaoui/rtic-mc-experiments/wiki).