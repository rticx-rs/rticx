# RTIC Evolution: Multicore support, Distributions and compilations passes

**Project Objective:**

The objective of this project is to enhance the scalability of the RTIC (Real-Time Interrupt-driven Concurrency) framework, particularly to make it flexible enough to allow supporting different kinds of multi-core hardware configurations. The goal is to:

- Find ways to reduce the RTIC codebase complexity
- Decouple the generic RTIC proc-macro logic from the hardware-specific implementation details
- Define good abstractions that facilitate targeting different kinds of hardware architectures.
- Provide means for extending RTIC syntax using external rust crates

By achieving this, we aim to create a more maintainable and extensible RTIC framework capable of accommodating various hardware configurations.

## How the project objectives have been achieved

The RTIC framework modularity problem can be solved like in any other software project by finding the right abstractions, and with simple software design patterns.

- First, Decoupling the RTIC declarative model code parsing and generation from the hardware-specific details has been achieved by carefully reviewing the current RTIC codebase, identifying which parts were dependent on hardware-specific details and which parts were more generic. After doing this, It turned out that only a small portion of the code generation was hardware-specific. So, this part was taken out and replaced by dynamically linking an external type that implements some trait functions that include the hardware-specific logic.

- The second part is about reducing the complexity of the RTIC project and making it more accessible to first-time contributors. This problem has been solved by devising an approach where the RTIC application is parsed and expanded several times. In essence, RTIC is all about having tasks that can share resources, with some of those tasks that can be bound to interrupt lines. So, a basic crate called `rtic-core` was developed to provide just that, and in addition, it provides a mechanism for stacking external logic from other crates that can turn more complex RTIC applications into something that lower level passes can understand.

Solving those two problems lead to what is known as **RTIC distributions and External Compilation passes**

- [Compilation passes](compilation_passes/compilation_passes.md)
- [RTIC distributions](distributions/rtic_distributions.md)

### Project structure

In this experiment project

- `rtic-core` is the library crate which:
  - contains a built-in compilation pass that captures the **Tasks and Resources syntax mode**  which will be referred to as **the core compilation pass**.
  - Exposes an Builder API for loading other external passes (from 3rd party crates) and for externally providing hardware specific implementations to build an RTIC framework

- `rtic-sw-pass` is the default crate that provides a software tasks pass. It does that by simply generating the necessary queues for message passing and then declaring the dispatchers as hardware tasks. Resource management and binding to interrupts and all other initialization steps will be taken care of by the hardware pass in`rtic-core`

- `rtic-deadline-pass` is a compilation pass that makes a simple "deadlines-to-priorities" conversion for tasks.

- `rp2040-rtic`: is an example RTIC distribution (multicore) specific to the RP2040 which defines the rp2040 specific hardware details and provides them to  `rtic-core` , `rtic-sw-pass` and other compilation passes crates to create the desired distribution.  

- `stm32-renode-rtic`: Another multicore distribution targeting a renode simulation of a modified stm32f1c3 MCU architecture.

- `hippo-rtic`: an distribution targeting a single (soft-)core RISC-V MCU.

### More

- [Rust code documentation](https://zakimadaoui.github.io/rtic-mc-experiments/)

### Other useful links

- [single core rtic application example](rp2040-rtic/examples/hello_rtic.rs)
- [multi-core rtic application with cross-core communication (classic ping-pong)](rp2040-rtic/examples/ping_pong.rs)  
