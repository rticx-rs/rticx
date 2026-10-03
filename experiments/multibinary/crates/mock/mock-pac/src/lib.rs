//! Empty PAC stand-in for the shared mock distribution.
//!
//! The mock distribution generates no target-specific peripheral code, so
//! `#[app(device = mock_pac)]` only needs the crate to exist for the
//! generated `use mock_pac as _;`.
