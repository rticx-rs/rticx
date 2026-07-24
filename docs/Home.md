# RTIC Modular Rewrite Wiki

Welcome to the documentation for the modular rewrite of the RTIC (Real-Time Interrupt-driven Concurrency) framework for Rust.

This project separates the generic RTIC proc-macro logic into a core crate (`rtic-core`) and lets target-specific functionality live in pluggable **distributions**. New language features can be added as external **compilation passes** without modifying the core.

## Documentation for application developers

If you want to write an RTIC application using an existing distribution, start here:

- [User Guide](User-Guide)
- [Getting Started](User-Guide-Getting-Started)
- [Syntax Reference](User-Guide-Syntax)
- [Supported Distributions](User-Guide-Supported-Distributions)

## Documentation for distribution developers

If you want to port RTIC to new hardware or write a new compilation pass, start here:

- [Distributor Guide](Distributor-Guide)
- [Architecture](Distributor-Guide-Architecture)
- [Writing Compilation Passes](Distributor-Guide-Writing-Compilation-Passes)
- [Writing Distributions](Distributor-Guide-Writing-Distributions)
- [Multibin and Multipac](Distributor-Guide-Multibin-Multipac)

## Publications

- [Thesis / paper about this project](THESIS_URL_PLACEHOLDER)
- [Additional publications](PUBLICATIONS_URL_PLACEHOLDER)
